//! HidDevice against the in-memory dongle simulator (paused tokio clock).

use std::time::Duration;

use ap_core::mock::MockDevice;
use ap_core::{Button, Device, Key, Point, SwipeDirection};
use ap_hid::protocol::HostMsg;
use ap_hid::report::{mods, usage};
use ap_hid::{CalibrationProfile, HidDevice, Report};

async fn dev() -> (HidDevice, ap_hid::SimHandle) {
    HidDevice::simulated(CalibrationProfile::iphone_17_pro())
        .await
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn tap_bytes_and_timing() {
    let (d, sim) = dev().await;
    d.tap(Point::new(0.5, 0.25)).await.unwrap();
    let rec = sim.recorded();
    let bytes: Vec<Vec<u8>> = rec.iter().map(|r| r.report.to_bytes()).collect();
    // x = round(0.5 * 32767) = 16384 = 0x4000, y = round(0.25 * 32767) = 8192 = 0x2000
    assert_eq!(
        bytes,
        vec![
            vec![3, 0, 0x00, 0x40, 0x00, 0x20],
            vec![3, 1, 0x00, 0x40, 0x00, 0x20],
            vec![3, 0, 0x00, 0x40, 0x00, 0x20],
        ]
    );
    // Unknown start: 0.2 s settle before the press, 60 ms hold.
    assert_eq!(rec[1].at - rec[0].at, Duration::from_millis(200));
    assert_eq!(rec[2].at - rec[1].at, Duration::from_millis(60));

    // Second tap elsewhere: the position is known, so the settle is 0.1 s.
    sim.clear();
    d.tap(Point::new(0.2, 0.8)).await.unwrap();
    let rec = sim.recorded();
    assert_eq!(rec.len(), 3);
    assert_eq!(rec[1].at - rec[0].at, Duration::from_millis(100));

    // Same place again: no move at all, press at once.
    sim.clear();
    d.tap(Point::new(0.2, 0.8)).await.unwrap();
    assert_eq!(
        sim.reports(),
        vec![Report::abs(0.2, 0.8, true), Report::abs(0.2, 0.8, false)]
    );
}

#[tokio::test(start_paused = true)]
async fn stale_pointer_waits_longer() {
    let (d, sim) = dev().await;
    d.tap(Point::new(0.5, 0.5)).await.unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    sim.clear();
    d.tap(Point::new(0.5, 0.5)).await.unwrap();
    let rec = sim.recorded();
    assert_eq!(rec.len(), 3, "stale position: hover again");
    assert_eq!(rec[1].at - rec[0].at, Duration::from_millis(200));
}

#[tokio::test(start_paused = true)]
async fn swipe_sequence() {
    let (d, sim) = dev().await;
    d.swipe(
        Point::new(0.5, 0.8),
        Point::new(0.5, 0.2),
        Duration::from_millis(300),
    )
    .await
    .unwrap();
    let rec = sim.recorded();
    let n = rec.len();
    assert_eq!(rec[0].report, Report::abs(0.5, 0.8, false));
    assert_eq!(rec[1].report, Report::abs(0.5, 0.8, true));
    assert_eq!(rec[n - 1].report, Report::abs(0.5, 0.2, false));
    let rel = &rec[2..n - 1];
    assert!(rel.len() >= 300 / 8, "one report per tick: {}", rel.len());
    for (i, r) in rel.iter().enumerate() {
        let Report::Mouse {
            buttons, dx, dy, ..
        } = r.report
        else {
            panic!("only relative reports while held, got {:?}", r.report);
        };
        assert_eq!((buttons, dx), (0, 0));
        assert!(dy <= 0);
        if i > 0 {
            assert_eq!(r.at - rel[i - 1].at, Duration::from_millis(8));
        }
    }
    // Total motion: 0.6 * 874 = 524.4 pt, rounded down per report.
    let p = CalibrationProfile::iphone_17_pro();
    let moved: f32 = rel
        .iter()
        .map(|r| match r.report {
            Report::Mouse { dy, .. } => p.points_for(f32::from(dy.unsigned_abs())),
            _ => 0.0,
        })
        .sum();
    assert!((520.0..=524.5).contains(&moved), "moved {moved} pt");
}

#[tokio::test(start_paused = true)]
async fn scroll_and_back() {
    let (d, sim) = dev().await;
    d.scroll(Point::CENTER, SwipeDirection::Up, 0.4)
        .await
        .unwrap();
    let r = sim.reports();
    assert_eq!(r[1], Report::abs(0.5, 0.7, true));
    assert_eq!(*r.last().unwrap(), Report::abs(0.5, 0.3, false));
    sim.clear();
    d.back().await.unwrap();
    let r = sim.reports();
    assert_eq!(r[0], Report::abs(0.0, 0.5, false));
    assert_eq!(r[1], Report::abs(0.0, 0.5, true));
    assert_eq!(*r.last().unwrap(), Report::abs(0.8, 0.5, false));
}

#[tokio::test(start_paused = true)]
async fn keyboard_actions() {
    let (d, sim) = dev().await;
    d.button(Button::Home).await.unwrap();
    assert_eq!(
        sim.reports(),
        vec![Report::key(usage::H, mods::GUI), Report::KEYS_UP]
    );
    sim.clear();
    d.key(&Key::parse("cmd+space").unwrap()).await.unwrap();
    assert_eq!(sim.reports()[0].to_bytes(), [1, 0x08, 0, 44, 0, 0, 0, 0, 0]);
    sim.clear();
    d.type_text("Hi\n").await.unwrap();
    assert_eq!(
        sim.reports(),
        vec![
            Report::key(11, mods::SHIFT),
            Report::KEYS_UP,
            Report::key(12, 0),
            Report::KEYS_UP,
            Report::key(usage::ENTER, 0),
            Report::KEYS_UP,
        ]
    );
    sim.clear();
    d.toggle_keyboard().await.unwrap();
    assert_eq!(sim.reports()[0].to_bytes(), [4, 1]);
    assert!(
        d.button(Button::VolumeUp)
            .await
            .unwrap_err()
            .is_unsupported()
    );
}

#[tokio::test(start_paused = true)]
async fn open_app_sequence() {
    let (d, sim) = dev().await;
    d.launch_app("com.apple.Preferences").await.unwrap();
    let r = sim.reports();
    assert_eq!(r[0], Report::key(usage::H, mods::GUI));
    assert_eq!(r[2], Report::key(usage::SPACE, mods::GUI));
    assert_eq!(r[4], Report::key(22, mods::SHIFT)); // 'S' of "Settings"
    assert_eq!(r[r.len() - 2], Report::key(usage::ENTER, 0));
}

#[tokio::test(start_paused = true)]
async fn screenshot_needs_a_screen_source() {
    let (d, _sim) = dev().await;
    assert!(!d.capabilities().screenshot);
    let e = d.screenshot().await.unwrap_err();
    assert!(e.hint().unwrap().contains("with_screen"));

    let (d, sim) = dev().await;
    let d = d.with_screen_device(Box::new(MockDevice::new()));
    assert!(d.capabilities().screenshot);
    d.screenshot().await.unwrap();
    assert!(!d.info().screen_pixels.is_empty());
    // Input still goes to the dongle.
    d.tap(Point::CENTER).await.unwrap();
    assert_eq!(sim.reports().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn failure_mid_gesture_releases() {
    let (d, sim) = dev().await;
    sim.drop_replies(3); // the hover reply and the next two are lost
    assert!(d.tap(Point::CENTER).await.is_err());
    assert!(
        sim.host_msgs().contains(&HostMsg::ReleaseAll),
        "{:?}",
        sim.host_msgs()
    );
}

#[tokio::test(start_paused = true)]
async fn phone_not_connected_is_setup_error() {
    let (d, sim) = dev().await;
    sim.set_phone_connected(false);
    assert!(!d.phone_connected().await.unwrap());
    let e = d.tap(Point::CENTER).await.unwrap_err();
    assert!(e.hint().is_some(), "{e}");
    assert!(d.tap(Point::new(1.5, 0.0)).await.is_err());
}
