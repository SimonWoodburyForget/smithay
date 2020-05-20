//!
//! Support to register an [`EglDevice`](EglDevice)
//! to an open [`Session`](::backend::session::Session).
//!

use drm::control::{connector, crtc, Mode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use super::{EglDevice, EglSurfaceInternal};
use crate::backend::drm::{Device, Surface};
use crate::backend::egl::{
    ffi,
    native::{Backend, NativeDisplay, NativeSurface},
};
use crate::{
    backend::session::Signal as SessionSignal,
    signaling::{Linkable, Signaler},
};

/// [`SessionObserver`](SessionObserver)
/// linked to the [`EglDevice`](EglDevice) it was
/// created from.
pub struct EglDeviceObserver<N: NativeSurface + Surface> {
    backends: Weak<RefCell<HashMap<crtc::Handle, Weak<EglSurfaceInternal<N>>>>>,
}

impl<B, D> Linkable<SessionSignal> for EglDevice<B, D>
where
    B: Backend<Surface = <D as Device>::Surface> + 'static,
    D: Device
        + NativeDisplay<
            B,
            Arguments = (crtc::Handle, Mode, Vec<connector::Handle>),
            Error = <<D as Device>::Surface as Surface>::Error,
        > + Linkable<SessionSignal>
        + 'static,
    <D as Device>::Surface: NativeSurface,
{
    fn link(&mut self, signaler: Signaler<SessionSignal>) {
        self.dev.borrow_mut().link(signaler.clone());
        let mut observer = EglDeviceObserver {
            backends: Rc::downgrade(&self.backends),
        };

        let token = signaler.register(move |signal| match signal {
            SessionSignal::ActivateSession | SessionSignal::ActivateDevice { .. } => observer.activate(),
            _ => {}
        });

        self.links.push(token);
    }
}

impl<N: NativeSurface + Surface> EglDeviceObserver<N> {
    fn activate(&mut self) {
        if let Some(backends) = self.backends.upgrade() {
            for (_crtc, backend) in backends.borrow().iter() {
                if let Some(backend) = backend.upgrade() {
                    let old_surface = backend.surface.surface.replace(std::ptr::null());
                    if !old_surface.is_null() {
                        unsafe {
                            ffi::egl::DestroySurface(**backend.surface.display, old_surface as *const _);
                        }
                    }
                }
            }
        }
    }
}
