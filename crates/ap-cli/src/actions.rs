//! Single phone actions: screenshot, tap, swipe, type, ...

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ap_core::{Device, DeviceInfo, Key, Platform, Point, Size, SwipeDirection};
use serde_json::json;

use crate::cli::ScrollDir;
use crate::ui;

/// Turn user numbers into a normalized point. With `px`, the numbers are
/// screenshot pixels.
pub fn to_point(x: f32, y: f32, px: bool, pixels: Size) -> Result<Point> {
    if !x.is_finite() || !y.is_finite() {
        bail!("coordinates must be numbers");
    }
    if px {
        if pixels.is_empty() {
            bail!("the screen size is unknown, so --px cannot be used here; give fractions 0..1");
        }
        let p = Point::from_absolute(x, y, pixels);
        if !p.is_valid() {
            bail!(
                "({x}, {y}) is outside the screen ({} x {} pixels)",
                pixels.width,
                pixels.height
            );
        }
        return Ok(p);
    }
    let p = Point::new(x, y);
    if !p.is_valid() {
        if x > 1.0 || y > 1.0 {
            bail!(
                "({x}, {y}) is outside 0..1. Coordinates are fractions of the screen (0.5 0.5 = centre); add --px to give pixels"
            );
        }
        bail!("({x}, {y}) is outside 0..1");
    }
    Ok(p)
}

/// Screenshot pixel size, from a fresh screenshot. `--px` means pixels of
/// the saved screenshot, and the device info size can be a guess (for
/// example an unknown iPad model). The device info is the fallback when the
/// phone cannot take a screenshot.
pub async fn pixel_size(d: &dyn Device) -> Result<Size> {
    match d.screenshot().await {
        Ok(f) if !f.size().is_empty() => Ok(f.size()),
        Ok(_) => Ok(d.info().screen_pixels),
        Err(e) => {
            let s = d.info().screen_pixels;
            if s.is_empty() {
                return Err(e.into());
            }
            Ok(s)
        }
    }
}

pub async fn point(d: &dyn Device, x: f32, y: f32, px: bool) -> Result<Point> {
    let size = if px {
        pixel_size(d).await?
    } else {
        Size::default()
    };
    to_point(x, y, px, size)
}

fn done(json: bool, what: &str, extra: serde_json::Value) -> Result<()> {
    if json {
        let mut v = json!({ "ok": true, "action": what });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        ui::json(&v)
    } else {
        ui::ok(what);
        Ok(())
    }
}

pub fn print_info(info: &DeviceInfo, caps: ap_core::Capabilities, json: bool) -> Result<()> {
    if json {
        return ui::json(&json!({ "device": info, "capabilities": caps.names() }));
    }
    println!("{}", console::style(&info.name).bold());
    println!("  id:        {}", info.id);
    println!("  platform:  {} {}", info.platform, info.os_version);
    println!("  model:     {}", info.model);
    println!(
        "  screen:    {} x {} {} ({} x {} px)",
        info.screen_points.width,
        info.screen_points.height,
        if info.platform == Platform::Ios {
            "pt"
        } else {
            "dp"
        },
        info.screen_pixels.width,
        info.screen_pixels.height
    );
    println!("  backend:   {}", info.backend);
    println!("  can do:    {}", caps.names().join(", "));
    Ok(())
}

pub async fn screenshot(d: &dyn Device, out: &Path, grid: bool, json: bool) -> Result<()> {
    let mut f = d.screenshot().await?;
    if grid {
        ap_screen::draw_grid(&mut f, &ap_screen::GridOptions::default());
    }
    let png = f.to_png()?;
    std::fs::write(out, &png).with_context(|| format!("cannot write {}", out.display()))?;
    if json {
        return ui::json(&json!({
            "ok": true,
            "path": out.display().to_string(),
            "width": f.width,
            "height": f.height,
        }));
    }
    ui::ok(format!(
        "saved {} ({} x {} px)",
        out.display(),
        f.width,
        f.height
    ));
    Ok(())
}

pub async fn tap(d: &dyn Device, at: Point, double: bool, json: bool) -> Result<()> {
    if double {
        d.double_tap(at).await?;
    } else {
        d.tap(at).await?;
    }
    let what = if double { "double-tapped" } else { "tapped" };
    done(
        json,
        &format!("{what} ({:.3}, {:.3})", at.x, at.y),
        json!({ "x": at.x, "y": at.y }),
    )
}

pub async fn tap_on(d: &dyn Device, text: &str, exact: bool, json: bool) -> Result<()> {
    let recognizer = ap_vision::default_recognizer()?;
    // Read a settled screen: text read during an animation can move before
    // the tap lands. Try twice, then stop without tapping.
    let mut settled = ap_screen::settle_with(d, ap_screen::SettleConfig::default()).await?;
    if !settled.still {
        settled = ap_screen::settle_with(d, ap_screen::SettleConfig::default()).await?;
    }
    if !settled.still {
        bail!(
            "the screen is still changing, so the text position is not sure. Wait, then run tap-on again"
        );
    }
    let frame = settled.frame;
    let boxes = recognizer.recognize(&frame)?;
    let lines = ap_vision::screen_lines(&boxes, false);
    let opts = ap_vision::FindOptions {
        fuzzy: !exact,
        ..Default::default()
    };
    let Some(hit) = ap_vision::find_text_box(&lines, text, &opts) else {
        let seen: Vec<&str> = lines.iter().take(15).map(|b| b.text.as_str()).collect();
        bail!(
            "\"{text}\" is not on the screen. Text found: {}",
            if seen.is_empty() {
                "none".to_string()
            } else {
                seen.join(" | ")
            }
        );
    };
    let at = hit.center();
    d.tap(at).await?;
    done(
        json,
        &format!("tapped \"{}\" at ({:.3}, {:.3})", hit.text, at.x, at.y),
        json!({ "x": at.x, "y": at.y, "text": hit.text }),
    )
}

pub async fn long_press(d: &dyn Device, at: Point, ms: u64, json: bool) -> Result<()> {
    d.long_press(at, Duration::from_millis(ms)).await?;
    done(
        json,
        &format!("long-pressed ({:.3}, {:.3}) for {ms} ms", at.x, at.y),
        json!({ "x": at.x, "y": at.y, "ms": ms }),
    )
}

pub async fn swipe(d: &dyn Device, from: Point, to: Point, ms: u64, json: bool) -> Result<()> {
    d.swipe(from, to, Duration::from_millis(ms)).await?;
    done(
        json,
        &format!(
            "swiped ({:.3}, {:.3}) -> ({:.3}, {:.3})",
            from.x, from.y, to.x, to.y
        ),
        json!({ "from": from, "to": to, "ms": ms }),
    )
}

/// Finger direction for "scroll the content <dir>": to see what is further
/// down, the finger moves up.
pub fn finger_direction(dir: ScrollDir) -> SwipeDirection {
    match dir {
        ScrollDir::Down => SwipeDirection::Up,
        ScrollDir::Up => SwipeDirection::Down,
        ScrollDir::Left => SwipeDirection::Right,
        ScrollDir::Right => SwipeDirection::Left,
    }
}

pub async fn scroll(
    d: &dyn Device,
    dir: ScrollDir,
    amount: f32,
    at: Option<Vec<f32>>,
    json: bool,
) -> Result<()> {
    if !(0.05..=1.0).contains(&amount) {
        bail!("--amount must be between 0.05 and 1");
    }
    let at = match at.as_deref() {
        Some([x, y]) => to_point(*x, *y, false, Size::default())?,
        _ => Point::CENTER,
    };
    d.scroll(at, finger_direction(dir), amount).await?;
    done(
        json,
        &format!("scrolled {dir:?}").to_lowercase(),
        json!({ "amount": amount }),
    )
}

pub async fn type_text(d: &dyn Device, text: &str, json: bool) -> Result<()> {
    for (i, part) in text.split('\n').enumerate() {
        if i > 0 {
            d.key(&Key::Enter).await?;
        }
        if !part.is_empty() {
            d.type_text(part).await?;
        }
    }
    done(
        json,
        &format!("typed {} characters", text.chars().count()),
        json!({}),
    )
}

pub fn parse_key(s: &str) -> Result<Key> {
    Key::parse(s).with_context(|| {
        format!(
            "\"{s}\" is not a key. Examples: enter, backspace, tab, escape, space, up, a, cmd+space, ctrl+shift+left"
        )
    })
}

pub async fn key(d: &dyn Device, s: &str, json: bool) -> Result<()> {
    let k = parse_key(s)?;
    d.key(&k).await?;
    done(json, &format!("pressed {s}"), json!({ "key": s }))
}

pub async fn button(d: &dyn Device, b: ap_core::Button, json: bool) -> Result<()> {
    d.button(b).await?;
    done(json, &format!("pressed {b:?}"), json!({ "button": b }))
}

/// Bundle id / package for an app id or a plain name ("Settings").
pub async fn resolve_app(d: &dyn Device, app: &str) -> Result<String> {
    if app.contains('.') && !app.contains(char::is_whitespace) {
        return Ok(app.to_string());
    }
    let ios = d.info().platform == Platform::Ios;
    if let Some(id) = ap_agent::tools::app_alias(app, ios) {
        return Ok(id.to_string());
    }
    let want: String = app
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if let Ok(apps) = d.list_apps().await {
        let hits: Vec<&String> = apps
            .iter()
            .filter(|id| {
                id.rsplit('.')
                    .next()
                    .is_some_and(|last| last.to_lowercase() == want)
            })
            .collect();
        if let [one] = hits.as_slice() {
            return Ok((*one).clone());
        }
    }
    bail!(
        "no app id known for \"{app}\". Run `agentphone apps` and give the bundle id (iOS) or package name (Android)"
    )
}

pub async fn open(d: &dyn Device, app: &str, json: bool) -> Result<()> {
    let id = resolve_app(d, app).await?;
    d.launch_app(&id).await?;
    done(json, &format!("opened {id}"), json!({ "app": id }))
}

pub async fn apps(d: &dyn Device, json: bool) -> Result<()> {
    let mut list = d.list_apps().await?;
    list.sort();
    if json {
        return ui::json(&list);
    }
    for a in &list {
        println!("{a}");
    }
    eprintln!("{} apps", list.len());
    Ok(())
}

pub async fn ui_tree(d: &dyn Device, json: bool) -> Result<()> {
    let tree = d.ui_tree().await?;
    match (tree, json) {
        (Some(t), true) => ui::json(&json!({ "tree": t })),
        (Some(t), false) => {
            println!("{t}");
            Ok(())
        }
        (None, true) => ui::json(&json!({ "tree": null })),
        (None, false) => {
            ui::warn("this screen has no UI tree");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ap_core::Gesture;
    use ap_core::mock::MockDevice;

    #[test]
    fn coordinates() {
        let px = Size::new(1170.0, 2532.0);
        assert_eq!(
            to_point(0.5, 0.25, false, Size::default()).unwrap(),
            Point::new(0.5, 0.25)
        );
        let p = to_point(585.0, 1266.0, true, px).unwrap();
        assert!((p.x - 0.5).abs() < 1e-6 && (p.y - 0.5).abs() < 1e-6);
        let e = to_point(300.0, 600.0, false, px).unwrap_err().to_string();
        assert!(e.contains("--px"), "{e}");
        assert!(to_point(2000.0, 10.0, true, px).is_err());
        assert!(to_point(1.0, 1.0, true, Size::default()).is_err());
        assert!(to_point(f32::NAN, 0.0, false, px).is_err());
    }

    #[test]
    fn scroll_maps_to_finger() {
        assert_eq!(finger_direction(ScrollDir::Down), SwipeDirection::Up);
        assert_eq!(finger_direction(ScrollDir::Left), SwipeDirection::Right);
        assert!(parse_key("cmd+space").is_ok());
        assert!(parse_key("hyper+q").is_err());
    }

    #[tokio::test]
    async fn actions_reach_the_device() {
        let m = MockDevice::new();
        let d: &dyn Device = &m;
        tap(d, Point::CENTER, false, true).await.unwrap();
        let p = point(d, 1170.0, 2532.0 / 4.0, true).await.unwrap();
        tap(d, p, true, true).await.unwrap();
        scroll(d, ScrollDir::Down, 0.5, None, true).await.unwrap();
        type_text(d, "hi\nthere", true).await.unwrap();
        key(d, "cmd+space", true).await.unwrap();
        open(d, "com.apple.Preferences", true).await.unwrap();
        open(d, "mobilesafari", true).await.unwrap();
        assert!(open(d, "No Such App", true).await.is_err());
        let calls = m.calls();
        assert_eq!(calls[0], Gesture::Tap { at: Point::CENTER });
        assert!(
            matches!(calls[1], Gesture::DoubleTap { at } if (at.x - 1.0).abs() < 1e-6 && (at.y - 0.25).abs() < 1e-6)
        );
        assert!(matches!(
            calls[2],
            Gesture::Scroll {
                direction: SwipeDirection::Up,
                ..
            }
        ));
        assert_eq!(calls[3], Gesture::TypeText { text: "hi".into() });
        assert_eq!(calls[4], Gesture::Key { key: Key::Enter });
        assert_eq!(
            calls[5],
            Gesture::TypeText {
                text: "there".into()
            }
        );
        assert!(matches!(&calls[7], Gesture::LaunchApp { id } if id == "com.apple.Preferences"));
        assert!(matches!(&calls[8], Gesture::LaunchApp { id } if id == "com.apple.mobilesafari"));
    }

    #[tokio::test]
    async fn px_uses_the_screenshot_size() {
        let m = MockDevice::new();
        let d: &dyn Device = &m;
        let before = m.screenshot_count();
        let p = point(d, 585.0, 1266.0, true).await.unwrap();
        assert!((p.x - 0.5).abs() < 1e-6 && (p.y - 0.5).abs() < 1e-6);
        assert_eq!(m.screenshot_count(), before + 1);
        // Fractions need no screenshot.
        point(d, 0.5, 0.5, false).await.unwrap();
        assert_eq!(m.screenshot_count(), before + 1);
    }

    #[tokio::test]
    async fn screenshot_with_grid() {
        let m = MockDevice::new();
        let mut info = m.info().clone();
        info.screen_pixels = Size::new(100.0, 200.0);
        let m = m.with_info(info);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("s.png");
        screenshot(&m, &out, true, true).await.unwrap();
        let f = ap_core::Frame::from_encoded(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((f.width, f.height), (100, 200));
    }
}
