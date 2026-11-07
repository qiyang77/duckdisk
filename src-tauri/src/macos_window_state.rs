//! macOS displays share a logical point coordinate space. Persist points,
//! not the physical pixels used by the generic window-state plugin.

use serde::{Deserialize, Serialize};
use std::{error::Error, sync::Mutex};
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewWindow};

const FILENAME: &str = ".window-state-logical.json";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
struct Frame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Frame {
    fn valid(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 900.0
            && self.height >= 620.0
    }
}

struct Screen {
    frame: Frame,
    scale: f64,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct SavedWindow {
    version: u32,
    frame: Frame,
    maximized: bool,
}

#[derive(Deserialize)]
struct LegacyWindow {
    #[serde(flatten)]
    frame: Frame,
    #[serde(default)]
    maximized: bool,
}

struct NormalFrame(Mutex<SavedWindow>);

// Legacy files have no scale factor. Migrate only when one display explains
// the coordinates and a valid size; reject already-halved or ambiguous frames.
fn legacy_frame(physical: Frame, screens: &[Screen]) -> Option<Frame> {
    let mut matches = screens.iter().filter_map(|screen| {
        if !screen.scale.is_finite() || screen.scale <= 0.0 {
            return None;
        }
        let frame = Frame {
            x: physical.x / screen.scale,
            y: physical.y / screen.scale,
            width: physical.width / screen.scale,
            height: physical.height / screen.scale,
        };
        let b = screen.frame;
        (frame.valid()
            && frame.width <= b.width
            && frame.height <= b.height
            && frame.x >= b.x
            && frame.x < b.x + b.width
            && frame.y >= b.y
            && frame.y < b.y + b.height - 40.0)
            .then_some(frame)
    });
    let frame = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(frame)
    }
}

fn record(window: &WebviewWindow) -> tauri::Result<()> {
    let maximized = window.is_maximized()?;
    window.state::<NormalFrame>().0.lock().unwrap().maximized = maximized;
    if maximized || window.is_minimized()? || window.is_fullscreen()? {
        return Ok(());
    }
    let scale = window
        .current_monitor()?
        .map(|m| m.scale_factor())
        .unwrap_or(window.scale_factor()?);
    let position = window.outer_position()?.to_logical::<f64>(scale);
    let size = window.inner_size()?.to_logical::<f64>(scale);
    let frame = Frame {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    if frame.valid() {
        window.state::<NormalFrame>().0.lock().unwrap().frame = frame;
    }
    Ok(())
}

pub fn restore(app: &tauri::AppHandle) -> Result<(), Box<dyn Error>> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window unavailable during restore")?;
    let screens: Vec<_> = window
        .available_monitors()?
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let position = monitor.position().to_logical::<f64>(scale);
            let size = monitor.size().to_logical::<f64>(scale);
            Screen {
                frame: Frame {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                },
                scale,
            }
        })
        .collect();
    let config = app.config();
    let config = config.app.windows.iter().find(|w| w.label == "main");
    let mut frame = Frame {
        x: 0.0,
        y: 0.0,
        width: config.map_or(1200.0, |w| w.width),
        height: config.map_or(760.0, |w| w.height),
    };
    if let Some(primary) = window.primary_monitor()? {
        let size = primary.size().to_logical::<f64>(primary.scale_factor());
        let position = primary.position().to_logical::<f64>(primary.scale_factor());
        frame.x = position.x + (size.width - frame.width) / 2.0;
        frame.y = position.y + (size.height - frame.height) / 2.0;
    }
    let dir = app.path().app_config_dir()?;
    let saved = std::fs::read(dir.join(FILENAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedWindow>(&bytes).ok())
        .filter(|saved| saved.version == 1 && saved.frame.valid())
        .or_else(|| {
            let bytes = std::fs::read(dir.join(".window-state.json")).ok()?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            let legacy: LegacyWindow = serde_json::from_value(value.get("main")?.clone()).ok()?;
            Some(SavedWindow {
                version: 1,
                frame: legacy_frame(legacy.frame, &screens)?,
                maximized: legacy.maximized,
            })
        });
    let maximized = saved.as_ref().is_some_and(|saved| saved.maximized);
    if let Some(saved) = saved {
        frame.width = saved.frame.width;
        frame.height = saved.frame.height;
        // A disconnected display must not leave the title bar offscreen.
        if screens.iter().any(|screen| {
            let b = screen.frame;
            saved.frame.x >= b.x
                && saved.frame.x < b.x + b.width - 40.0
                && saved.frame.y >= b.y
                && saved.frame.y < b.y + b.height - 40.0
        }) {
            frame.x = saved.frame.x;
            frame.y = saved.frame.y;
        }
    }
    app.manage(NormalFrame(Mutex::new(SavedWindow {
        version: 1,
        frame,
        maximized,
    })));
    window.set_position(LogicalPosition::new(frame.x, frame.y))?;
    window.set_size(LogicalSize::new(frame.width, frame.height))?;
    if maximized {
        window.maximize()?;
    }
    let tracked_window = window.clone();
    window.on_window_event(move |event| {
        if matches!(
            event,
            tauri::WindowEvent::Moved(_)
                | tauri::WindowEvent::Resized(_)
                | tauri::WindowEvent::ScaleFactorChanged { .. }
        ) {
            let _ = record(&tracked_window);
        }
    });
    Ok(())
}

// Snapshot before the graceful exit starts, while the native window still exists.
pub fn save(app: &tauri::AppHandle) -> Result<(), Box<dyn Error>> {
    let normal = app
        .try_state::<NormalFrame>()
        .ok_or("Window geometry was not initialized")?;
    // Tauri can remove the window from its map before ExitRequested (Command-Q).
    // The move/resize cache remains valid even when the handle is no longer listed.
    if let Some(window) = app.get_webview_window("main") {
        record(&window)?;
    }
    let saved = *normal.0.lock().unwrap();
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    let temporary = dir.join(format!("{FILENAME}.tmp"));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&saved)?)?;
    std::fs::rename(temporary, dir.join(FILENAME))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screens() -> Vec<Screen> {
        vec![
            Screen {
                frame: Frame {
                    x: 0.0,
                    y: 0.0,
                    width: 1352.0,
                    height: 878.0,
                },
                scale: 2.0,
            },
            Screen {
                frame: Frame {
                    x: 1352.0,
                    y: -202.0,
                    width: 1920.0,
                    height: 1080.0,
                },
                scale: 1.0,
            },
        ]
    }
    #[test]
    fn external_geometry_is_not_halved_on_retina_startup() {
        let frame = Frame {
            x: 1449.0,
            y: -35.0,
            width: 1100.0,
            height: 700.0,
        };
        assert_eq!(legacy_frame(frame, &screens()), Some(frame));
    }
    #[test]
    fn retina_geometry_converts_to_points_once() {
        let frame = Frame {
            x: 200.0,
            y: 80.0,
            width: 2200.0,
            height: 1400.0,
        };
        assert_eq!(
            legacy_frame(frame, &screens()),
            Some(Frame {
                x: 100.0,
                y: 40.0,
                width: 1100.0,
                height: 700.0
            })
        );
    }
    #[test]
    fn corrupted_or_disconnected_legacy_geometry_is_ignored() {
        assert_eq!(
            legacy_frame(
                Frame {
                    x: 1532.0,
                    y: 30.0,
                    width: 861.0,
                    height: 443.0
                },
                &screens()
            ),
            None
        );
        assert_eq!(
            legacy_frame(
                Frame {
                    x: 9000.0,
                    y: 0.0,
                    width: 1100.0,
                    height: 700.0
                },
                &screens()
            ),
            None
        );
    }
    #[test]
    fn logical_geometry_round_trips_without_scaling() {
        let frame = Frame {
            x: 1500.0,
            y: 60.0,
            width: 1100.0,
            height: 700.0,
        };
        let saved = SavedWindow {
            version: 1,
            frame,
            maximized: false,
        };
        let decoded: SavedWindow =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(decoded.frame, frame);
    }
}
