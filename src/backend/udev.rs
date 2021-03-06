//!
//! Provides `udev` related functionality for automated device scanning.
//!
//! This module mainly provides the [`UdevBackend`](::backend::udev::UdevBackend), which
//! monitors available DRM devices and acts as an event source, generating events whenever these
//! devices change.
//!
//! *Note:* Once inserted into the event loop, the [`UdevBackend`](::backend::udev::UdevBackend) will
//! only notify you about *changes* in the device list. To get an initial snapshot of the state during
//! your initialization, you need to call its `device_list` method.
//!
//! ```no_run
//! use smithay::backend::udev::{UdevBackend, UdevEvent};
//!
//! let udev = UdevBackend::new("seat0", None).expect("Failed to monitor udev.");
//!
//! for (dev_id, node_path) in udev.device_list() {
//!     // process the initial list of devices
//! }
//!
//! # let event_loop = smithay::reexports::calloop::EventLoop::<()>::new().unwrap();
//! # let loop_handle = event_loop.handle();
//! // setup the event source for long-term monitoring
//! loop_handle.insert_source(udev, |event, _, _dispatch_data| match event {
//!     UdevEvent::Added { device_id, path } => {
//!         // a new device has been added
//!     },
//!     UdevEvent::Changed { device_id } => {
//!         // a device has been changed
//!     },
//!     UdevEvent::Removed { device_id } => {
//!         // a device has been removed
//!     }
//! }).expect("Failed to insert the udev source into the event loop");
//! ```
//!
//! Additionally this contains some utility functions related to scanning.
//!
//! See also `anvil/src/udev.rs` for pure hardware backed example of a compositor utilizing this
//! backend.

use nix::sys::stat::{dev_t, stat};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Result as IoResult,
    os::unix::io::{AsRawFd, RawFd},
    path::{Path, PathBuf},
};
use udev::{Enumerator, EventType, MonitorBuilder, MonitorSocket};

use calloop::{EventSource, Interest, Mode, Poll, Readiness, Token};

/// Backend to monitor available drm devices.
///
/// Provides a way to automatically scan for available gpus and notifies the
/// given handler of any changes. Can be used to provide hot-plug functionality for gpus and
/// attached monitors.
pub struct UdevBackend {
    devices: HashMap<dev_t, PathBuf>,
    monitor: MonitorSocket,
    logger: ::slog::Logger,
}

impl AsRawFd for UdevBackend {
    fn as_raw_fd(&self) -> RawFd {
        self.monitor.as_raw_fd()
    }
}

impl UdevBackend {
    /// Creates a new [`UdevBackend`]
    ///
    /// ## Arguments
    /// `seat`    - system seat which should be bound
    /// `logger`  - slog Logger to be used by the backend and its `DrmDevices`.
    pub fn new<L, S: AsRef<str>>(seat: S, logger: L) -> IoResult<UdevBackend>
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_udev"));

        let devices = all_gpus(seat)?
            .into_iter()
            // Create devices
            .flat_map(|path| match stat(&path) {
                Ok(stat) => Some((stat.st_rdev, path)),
                Err(err) => {
                    warn!(log, "Unable to get id of {:?}, Error: {:?}. Skipping", path, err);
                    None
                }
            })
            .collect();

        let monitor = MonitorBuilder::new()?.match_subsystem("drm")?.listen()?;

        Ok(UdevBackend {
            devices,
            monitor,
            logger: log,
        })
    }

    /// Get a list of DRM devices currently known to the backend
    ///
    /// You should call this once before inserting the event source into your
    /// event loop, to get an initial snapshot of the device state.
    pub fn device_list(&self) -> impl Iterator<Item = (dev_t, &Path)> {
        self.devices.iter().map(|(&id, path)| (id, path.as_ref()))
    }
}

impl EventSource for UdevBackend {
    type Event = UdevEvent;
    type Metadata = ();
    type Ret = ();

    fn process_events<F>(&mut self, _: Readiness, _: Token, mut callback: F) -> std::io::Result<()>
    where
        F: FnMut(UdevEvent, &mut ()),
    {
        let monitor = self.monitor.clone();
        for event in monitor {
            debug!(
                self.logger,
                "Udev event: type={}, devnum={:?} devnode={:?}",
                event.event_type(),
                event.devnum(),
                event.devnode()
            );
            match event.event_type() {
                // New device
                EventType::Add => {
                    if let (Some(path), Some(devnum)) = (event.devnode(), event.devnum()) {
                        info!(self.logger, "New device: #{} at {}", devnum, path.display());
                        if self.devices.insert(devnum, path.to_path_buf()).is_none() {
                            callback(
                                UdevEvent::Added {
                                    device_id: devnum,
                                    path: path.to_path_buf(),
                                },
                                &mut (),
                            );
                        }
                    }
                }
                // Device removed
                EventType::Remove => {
                    if let Some(devnum) = event.devnum() {
                        info!(self.logger, "Device removed: #{}", devnum);
                        if self.devices.remove(&devnum).is_some() {
                            callback(UdevEvent::Removed { device_id: devnum }, &mut ());
                        }
                    }
                }
                // New connector
                EventType::Change => {
                    if let Some(devnum) = event.devnum() {
                        info!(self.logger, "Device changed: #{}", devnum);
                        if self.devices.contains_key(&devnum) {
                            callback(UdevEvent::Changed { device_id: devnum }, &mut ());
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn register(&mut self, poll: &mut Poll, token: Token) -> std::io::Result<()> {
        poll.register(self.as_raw_fd(), Interest::Readable, Mode::Level, token)
    }

    fn reregister(&mut self, poll: &mut Poll, token: Token) -> std::io::Result<()> {
        poll.reregister(self.as_raw_fd(), Interest::Readable, Mode::Level, token)
    }

    fn unregister(&mut self, poll: &mut Poll) -> std::io::Result<()> {
        poll.unregister(self.as_raw_fd())
    }
}

/// Events generated by the [`UdevBackend`], notifying you of changes in system devices
pub enum UdevEvent {
    /// A new device has been detected
    Added {
        /// ID of the new device
        device_id: dev_t,
        /// Path of the new device
        path: PathBuf,
    },
    /// A device has changed
    Changed {
        /// ID of the changed device
        device_id: dev_t,
    },
    /// A device has been removed
    Removed {
        /// ID of the removed device
        device_id: dev_t,
    },
}

/// Returns the path of the primary GPU device if any
///
/// Might be used for filtering in [`UdevHandler::device_added`] or for manual
/// [`LegacyDrmDevice`](::backend::drm::legacy::LegacyDrmDevice) initialization.
pub fn primary_gpu<S: AsRef<str>>(seat: S) -> IoResult<Option<PathBuf>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;

    if let Some(path) = enumerator
        .scan_devices()?
        .filter(|device| {
            let seat_name = device
                .property_value("ID_SEAT")
                .map(|x| x.to_os_string())
                .unwrap_or_else(|| OsString::from("seat0"));
            if seat_name == *seat.as_ref() {
                if let Ok(Some(pci)) = device.parent_with_subsystem(Path::new("pci")) {
                    if let Some(id) = pci.attribute_value("boot_vga") {
                        return id == "1";
                    }
                }
            }
            false
        })
        .flat_map(|device| device.devnode().map(PathBuf::from))
        .next()
    {
        Ok(Some(path))
    } else {
        all_gpus(seat).map(|all| all.into_iter().next())
    }
}

/// Returns the paths of all available GPU devices
///
/// Might be used for manual  [`LegacyDrmDevice`](::backend::drm::legacy::LegacyDrmDevice)
/// initialization.
pub fn all_gpus<S: AsRef<str>>(seat: S) -> IoResult<Vec<PathBuf>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("drm")?;
    enumerator.match_sysname("card[0-9]*")?;
    Ok(enumerator
        .scan_devices()?
        .filter(|device| {
            device
                .property_value("ID_SEAT")
                .map(|x| x.to_os_string())
                .unwrap_or_else(|| OsString::from("seat0"))
                == *seat.as_ref()
        })
        .flat_map(|device| device.devnode().map(PathBuf::from))
        .collect())
}
