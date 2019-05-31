#![warn(rust_2018_idioms)]

#[macro_use]
extern crate slog;

use slog::Drain;
use smithay::{
    backend::{
        drm::{
            device_bind,
            common::Error as DrmError,
            //atomic::AtomicDrmDevice,
            legacy::LegacyDrmDevice,
            eglstream::{EglStreamDevice, EglStreamSurface, egl::EglStreamDeviceBackend, Error as EglStreamError},
            egl::{Error as EglError, EglDevice, EglSurface},
            Device, DeviceHandler,
        },
        graphics::glium::GliumGraphicsBackend,
    },
    reexports::{
        calloop::EventLoop,
        drm::{
            control::{
                connector::{State as ConnectorState},
                crtc,
            },
        },
    },
};
use glium::Surface as GliumSurface;
use std::{
    fs::{File, OpenOptions},
    io::Error as IoError,
    rc::Rc,
    sync::Mutex,
};

fn main() {
    let log = slog::Logger::root(Mutex::new(slog_term::term_full().fuse()).fuse(), o!());

    /*
     * Initialize the drm backend
     */

    // "Find" a suitable drm device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let mut device = 
        EglDevice::new(
            EglStreamDevice::new(
                LegacyDrmDevice::new(options.open("/dev/dri/card1").unwrap(), true, log.clone())
                    .expect("Failed to initialize drm device"),
                log.clone()
            ).expect("Failed to initialize egl stream device"),
            log.clone()
        ).expect("Failed to initialize egl device");

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = Device::resource_handles(&device).unwrap();

    // Use first connected connector
    let connector_info = res_handles
        .connectors()
        .iter()
        .map(|conn| Device::get_connector_info(&device, *conn).unwrap())
        .find(|conn| conn.state() == ConnectorState::Connected)
        .unwrap();
    println!("Conn: {:?}", connector_info.interface());

    // Use the first encoder
    let encoder_info = Device::get_encoder_info(&device, connector_info.encoders()[0].expect("expected encoder")).unwrap();

    // use the connected crtc if any
    let crtc = encoder_info
        .crtc()
        // or use the first one that is compatible with the encoder
        .unwrap_or_else(|| {
            *res_handles
                .filter_crtcs(encoder_info.possible_crtcs())
                .iter()
                .next()
                .unwrap()
        });
    println!("Crtc {:?}", crtc);

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resolution, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)
    println!("Mode: {:?}", mode);

    // Initialize the hardware backend
    let surface = device.create_surface(crtc, mode, &[connector_info.handle()]).expect("Failed to create surface");

    let backend: Rc<GliumGraphicsBackend<_>> = Rc::new(surface.into());
    device.set_handler(DrmHandlerImpl {
        surface: backend.clone(),
    });

    /*
     * Register the DrmDevice on the EventLoop
     */
    let mut event_loop = EventLoop::<()>::new().unwrap();
    let _source = device_bind(&event_loop.handle(), device)
        .map_err(|err| -> IoError { err.into() })
        .unwrap();

    // Start rendering
    {
        println!("Frame!");
        if let Err(err) = backend.draw().finish() { println!("{}", err) };
    }

    // Run
    event_loop.run(None, &mut (), |_| {}).unwrap();
}

pub struct DrmHandlerImpl {
    surface: Rc<GliumGraphicsBackend<EglSurface<EglStreamSurface<LegacyDrmDevice<File>>>>>,
}

impl DeviceHandler for DrmHandlerImpl {
    type Device = EglDevice<EglStreamDeviceBackend<LegacyDrmDevice<File>>, EglStreamDevice<LegacyDrmDevice<File>>>;

    fn vblank(&mut self, _crtc: crtc::Handle) {
        {
            println!("Vblank");
            let mut frame = self.surface.draw();
            frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);
            frame.finish().expect("Failed to swap");
        }
    }

    fn error(&mut self, error: EglError<EglStreamError<DrmError>>) {
        panic!("{:?}", error);
    }
}
