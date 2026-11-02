//! Integration and verification test suite for `open-3d-skills` (Rust edition).

use image::{Rgb, RgbImage, Rgba, RgbaImage};
use open_3d_skills::key::{key_frame, plate_colour, under_colour, KeyArgs};
use open_3d_skills::loop_check::mean_abs_diff;
use open_3d_skills::paste_back::{locate, PasteBackArgs};
use open_3d_skills::plate::{collisions, KeyColor, PlateArgs};
use open_3d_skills::record::{parse_scroll, pointer_at, scroll_at};
use open_3d_skills::serve::{handle_client, mime_type_for, parse_byte_range};
use open_3d_skills::site_check::{is_blank_region, write_contact_sheet, ShotReport};
use open_3d_skills::verify::verify_repo;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

#[test]
fn test_skills_manifests_and_scripts_verify() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (skills_count, manifests_count, scripts_count) =
        verify_repo(root).expect("repository verification should succeed");
    assert_eq!(skills_count, 8, "expected all 8 skills to load");
    assert_eq!(manifests_count, 5, "expected all 5 plugin manifests to agree");
    assert!(
        scripts_count >= 18,
        "expected all Rust & JS skill scripts to be verified, got {}",
        scripts_count
    );
}

#[test]
fn test_plate_and_key_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let subject_path = tmp.path().join("subject.png");
    let plate_path = tmp.path().join("plate.png");
    let png_out_dir = tmp.path().join("keyed_png");
    let frames_out_dir = tmp.path().join("keyed_frames");

    // Create a synthetic 120x160 RGBA cutout subject (warm orange subject in the centre, transparent border)
    let mut subj = RgbaImage::from_pixel(120, 160, Rgba([0, 0, 0, 0]));
    for y in 30..130 {
        for x in 35..85 {
            subj.put_pixel(x, y, Rgba([220, 140, 60, 255]));
        }
    }
    subj.save(&subject_path).unwrap();

    let rep = collisions(&subj);
    assert_eq!(rep.len(), 3);
    assert!(rep[0].1 < 0.5, "warm orange subject should not collide with magenta");

    open_3d_skills::plate::run(PlateArgs {
        subject: subject_path,
        out: plate_path.clone(),
        key: "auto".to_string(),
        size: "240x320".to_string(),
        fit: 0.75,
        anchor: "bottom".to_string(),
        margin: 0.05,
    })
    .expect("plate generation should succeed");

    assert!(plate_path.exists());
    let plate_im = image::open(&plate_path).unwrap().to_rgb8();
    assert_eq!(plate_im.dimensions(), (240, 320));

    let raw = plate_im.into_raw();
    let measured = plate_colour(&raw, 240, 320);
    assert!(measured[0] > 240.0 && measured[1] < 15.0 && measured[2] > 240.0);

    let (col, a, _) = key_frame(&raw, 240, 320, KeyColor::Magenta, 12.0, 0).unwrap();
    let under = under_colour(&col, &a, 240, 320, [128, 128, 128]);
    assert_eq!(under.len(), 240 * 320 * 3);

    open_3d_skills::key::run(KeyArgs {
        clip: plate_path,
        stacked: None,
        webm: None,
        frames: Some(frames_out_dir.clone()),
        png: Some(png_out_dir.clone()),
        key: "magenta".to_string(),
        frame_list: "0".to_string(),
        width: Some(120),
        fps: None,
        start: 0,
        end: None,
        step: 1,
        picks: None,
        quality: 82,
        crf: 20,
        solid: 12.0,
        erode: 0,
        fill: "808080".to_string(),
    })
    .expect("keying still into PNG and WebP frames should succeed");

    assert!(png_out_dir.join("f000.png").exists());
    assert!(frames_out_dir.join("000.webp").exists());
    assert!(frames_out_dir.join("manifest.json").exists());
}

#[test]
fn test_paste_back_template_matching() {
    let tmp = tempfile::tempdir().unwrap();
    let orig_path = tmp.path().join("orig.png");
    let ext_path = tmp.path().join("ext.png");
    let out_path = tmp.path().join("out.png");

    let mut ext = RgbImage::new(200, 160);
    for (x, y, px) in ext.enumerate_pixels_mut() {
        let dx = x as i32 - 75;
        let dy = y as i32 - 55;
        let r2 = (dx * dx + dy * dy) as u32;
        let ring = ((r2 / 17) % 256) as u8;
        let grid = if (x / 9 + y / 11) % 2 == 0 { 180u8 } else { 40u8 };
        *px = Rgb([ring, grid,ring.wrapping_add(grid / 2)]);
    }
    let orig = image::imageops::crop_imm(&ext, 40, 30, 80, 60).to_image();
    orig.save(&orig_path).unwrap();
    ext.save(&ext_path).unwrap();

    let (s, x, y, score, _, tw, th) = locate(
        orig.as_raw(),
        80,
        60,
        ext.as_raw(),
        200,
        160,
    );
    assert!((s - 1.0).abs() < 0.08, "expected scale near 1.0, got {}", s);
    assert!((x as i32 - 40).abs() <= 2 && (y as i32 - 30).abs() <= 2);
    assert!(score > 0.95, "expected high NCC match score, got {}", score);
    assert_eq!((tw, th), (80, 60));

    open_3d_skills::paste_back::run(PasteBackArgs {
        original: orig_path,
        extended: ext_path,
        out: out_path.clone(),
        feather: 8,
        no_match: false,
    })
    .unwrap();
    assert!(out_path.exists());
}

#[test]
fn test_scroll_and_pointer_choreography() {
    let keys = parse_scroll("0:0 2:0 6:50% 10~:2vh", 1000.0, 4000.0).unwrap();
    assert_eq!(keys.len(), 4);
    assert_eq!(scroll_at(&keys, 0.0), Some(0.0));
    assert_eq!(scroll_at(&keys, 1.5), Some(0.0));
    let mid = scroll_at(&keys, 4.0).unwrap();
    assert!((mid - 1000.0).abs() < 1e-3);
    assert_eq!(scroll_at(&keys, 10.0), Some(2000.0));

    assert_eq!(pointer_at("none", 1.0, 1920.0, 1080.0).unwrap(), None);
    let drift = pointer_at("drift", 2.0, 1920.0, 1080.0).unwrap().unwrap();
    assert!(drift.0 > 0.0 && drift.0 < 1920.0 && drift.1 > 0.0 && drift.1 < 1080.0);
    let fixed = pointer_at("0.25,0.75", 0.0, 1000.0, 800.0).unwrap().unwrap();
    assert_eq!(fixed, (250.0, 600.0));
}

#[test]
fn test_http_range_server() {
    assert_eq!(parse_byte_range("bytes=0-99", 500), Some(Ok((0, 99))));
    assert_eq!(parse_byte_range("bytes=400-", 500), Some(Ok((400, 499))));
    assert_eq!(parse_byte_range("bytes=-50", 500), Some(Ok((450, 499))));
    assert_eq!(parse_byte_range("bytes=600-700", 500), Some(Err(())));
    assert_eq!(mime_type_for(Path::new("scene.glb")), "model/gltf-binary");
    assert_eq!(mime_type_for(Path::new("clip.webm")), "video/webm");

    let tmp = tempfile::tempdir().unwrap();
    let video_path = tmp.path().join("sample.mp4");
    std::fs::write(&video_path, b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let root = tmp.path().to_path_buf();

    let handle = std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            handle_client(stream, &root).unwrap();
        }
    });

    let mut client = TcpStream::connect(addr).unwrap();
    client
        .write_all(b"GET /sample.mp4 HTTP/1.1\r\nHost: localhost\r\nRange: bytes=10-15\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    client.read_to_string(&mut resp).unwrap();
    handle.join().unwrap();

    assert!(resp.starts_with("HTTP/1.1 206 Partial Content\r\n"));
    assert!(resp.contains("Content-Range: bytes 10-15/36\r\n"));
    assert!(resp.ends_with("\r\n\r\nABCDEF"));
}

#[test]
fn test_site_check_blank_canvas_and_sheet() {
    let flat = RgbImage::from_pixel(100, 100, Rgb([40, 40, 40]));
    assert!(is_blank_region(&flat, 0, 0, 100, 100));

    let mut textured = RgbImage::from_pixel(100, 100, Rgb([40, 40, 40]));
    for y in 20..80 {
        for x in 20..80 {
            textured.put_pixel(x, y, Rgb([220, 180, 90]));
        }
    }
    assert!(!is_blank_region(&textured, 0, 0, 100, 100));
    assert!(mean_abs_diff(flat.as_raw(), textured.as_raw()) > 10.0);

    let tmp = tempfile::tempdir().unwrap();
    let shot_file = tmp.path().join("shot0.png");
    textured.save(&shot_file).unwrap();
    let reports = vec![ShotReport {
        name: "default-chromium-1440".to_string(),
        url: "http://127.0.0.1:8000/".to_string(),
        browser: "chromium".to_string(),
        width: 1440,
        reduced: false,
        scroll: None,
        ready_s: 0.42,
        ms_per_frame: 16.6,
        fps: Some(60.0),
        errors: vec![],
        blank_canvases: 0,
        overflow: false,
        file: shot_file.to_string_lossy().to_string(),
    }];
    write_contact_sheet(&reports, tmp.path()).unwrap();
    assert!(tmp.path().join("sheet.jpg").exists());
}

#[test]
fn test_installer_in_clean_home() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp_home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_home.path().join(".claude")).unwrap();
    std::fs::create_dir_all(tmp_home.path().join(".codex")).unwrap();

    let status = std::process::Command::new("bash")
        .arg(root.join("install.sh"))
        .env("HOME", tmp_home.path())
        .status()
        .expect("run install.sh");
    assert!(status.success(), "install.sh should succeed in clean HOME");

    assert!(
        tmp_home
            .path()
            .join(".claude/skills/threejs-scene")
            .symlink_metadata()
            .is_ok(),
        "expected symlink ~/.claude/skills/threejs-scene"
    );
    assert!(
        tmp_home
            .path()
            .join(".codex/skills/frame-capture")
            .symlink_metadata()
            .is_ok(),
        "expected symlink ~/.codex/skills/frame-capture"
    );

    let uninst = std::process::Command::new("bash")
        .arg(root.join("install.sh"))
        .args(["--uninstall", "--claude", "--codex"])
        .env("HOME", tmp_home.path())
        .status()
        .expect("run install.sh --uninstall");
    assert!(uninst.success(), "install.sh --uninstall should succeed");
    assert!(
        !tmp_home
            .path()
            .join(".claude/skills/threejs-scene")
            .exists(),
        "expected ~/.claude/skills/threejs-scene to be removed after --uninstall"
    );
}

#[test]
fn test_cursor_rig_dot_eyes_and_template_tracker() {
    use open_3d_skills::build_rig::{DotEyesTracker, TemplateTracker};

    // Create a 200x200 face canvas with two dark round dot eyes inside the head box (20, 20, 160, 160)
    let mut face = RgbImage::from_pixel(200, 200, Rgb([215, 165, 140]));
    for (ex, ey) in [(88i32, 80i32), (112i32, 80i32)] {
        for dy in -3..=3 {
            for dx in -3..=3 {
                if dx * dx + dy * dy <= 11 {
                    face.put_pixel((ex + dx) as u32, (ey + dy) as u32, Rgb([20, 20, 20]));
                }
            }
        }
    }

    let tracker = DotEyesTracker::new((20, 20, 160, 160), (50, 120), 70);
    let (mid, eyes) = tracker
        .track(face.as_raw(), 200, 200)
        .expect("DotEyesTracker should detect both dark round eyes");
    assert_eq!(eyes.len(), 2);
    assert!((mid.0 - 100.0).abs() < 2.0 && (mid.1 - 80.0).abs() < 2.0);

    let mut tpl = TemplateTracker::new((100.0, 80.0), 160);
    let p0 = tpl.track(face.as_raw(), 200, 200).unwrap();
    assert_eq!(p0, (100.0, 80.0));
    let p1 = tpl.track(face.as_raw(), 200, 200).unwrap();
    assert!((p1.0 - 100.0).abs() <= 1.0 && (p1.1 - 80.0).abs() <= 1.0);
}

