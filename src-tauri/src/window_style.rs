// Copyright 2020-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Add native macOS window shadows.
//!
//! # Example
//!
//! ```no_run
//! use window_shadows::set_shadow;
//!
//! # let window: &dyn raw_window_handle::HasRawWindowHandle = unsafe { std::mem::zeroed() };
//! set_shadow(&window, true).unwrap();
//! ```

/// Enables or disables the shadows for a window.
///
pub fn set_window_styles(window: impl raw_window_handle::HasRawWindowHandle) -> Result<(), Error> {
    match window.raw_window_handle() {
        raw_window_handle::RawWindowHandle::AppKit(handle) => {
            use cocoa::{
                appkit::{NSColor, NSWindow},
                base::{id, nil},
            };
            use objc::{
                msg_send,
                runtime::{NO, YES},
                sel, sel_impl,
            };

            unsafe {
                let window = handle.ns_window as id;
                window.setHasShadow_(YES);
                window.setOpaque_(NO);
                window.setBackgroundColor_(NSColor::clearColor(nil));

                let content_view: id = msg_send![window, contentView];
                let _: () = msg_send![content_view, setWantsLayer: YES];
                let layer: id = msg_send![content_view, layer];
                let _: () = msg_send![layer, setCornerRadius: 11.0_f64];
                let _: () = msg_send![layer, setMasksToBounds: YES];
            }

            Ok(())
        }
        _ => Err(Error::UnsupportedPlatform),
    }
}

#[derive(Debug)]
pub enum Error {
    UnsupportedPlatform,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\"set_shadow()\" is only supported on macOS")
    }
}
