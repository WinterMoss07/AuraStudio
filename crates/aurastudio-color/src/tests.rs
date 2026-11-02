use aurastudio_core::{ByteReader, ByteWriter, Rng};

use crate::*;

fn gray(c: [u8; 3]) -> bool { (c[0] as i32 - c[1] as i32).abs() <= 1 && (c[1] as i32 - c[2] as i32).abs() <= 1 }

fn setup(document: Profile, monitor: Profile, mode: ViewMode) -> DisplaySetup { DisplaySetup { document, monitor, intent: Intent::Perceptual, mode } }

#[test]
fn srgb_on_srgb_is_identity() {
    let s = DisplaySetup::plain();
    assert!(s.is_identity());
    let lut = s.build().unwrap();
    assert!(lut.is_identity());
    // A copy of sRGB from bytes is the same profile.
    let again = Profile::from_icc(Profile::srgb().icc(), "copy").unwrap();
    assert_eq!(again, Profile::srgb());
    assert!(again.is_srgb() && again.model() == Model::Rgb);
    assert!(Profile::srgb().description().contains("sRGB"));
}

/// A wide-gamut (Display P3) monitor: sRGB red sent as a less saturated
/// P3 red, grays exactly neutral; the table within one level of the
/// direct transform.
#[test]
fn wide_gamut_monitor_keeps_grays_neutral() {
    for intent in [Intent::Perceptual, Intent::RelativeColorimetric] {
        let s = DisplaySetup { intent, ..setup(Profile::srgb(), Profile::display_p3(), ViewMode::Normal) };
        let lut = s.build().unwrap();
        assert!(!lut.is_identity());
        let red = lut.apply_u8([255, 0, 0]);
        assert!(red[0] > 220 && red[1] > 30 && red[2] > 20, "{red:?}");
        for v in [32u8, 64, 128, 192, 240] { assert!(gray(lut.apply_u8([v; 3])), "gray {v}"); }
        let direct = Converter::new(&Profile::srgb(), &Profile::display_p3(), intent).unwrap();
        let mut rng = Rng::new(5);
        let mut worst = 0f32;
        for _ in 0..2000 {
            let c = [rng.unit(), rng.unit(), rng.unit()];
            let (a, b) = (lut.apply(c), direct.color(&c));
            for k in 0..3 { worst = worst.max((a[k] - b[k]).abs() * 255.0); }
        }
        assert!(worst < 1.0, "table off by {worst} levels");
    }
}

#[test]
fn grayscale_and_channel_views() {
    let lut = setup(Profile::srgb(), Profile::srgb(), ViewMode::Grayscale).build().unwrap();
    let (r, g, w) = (lut.apply_u8([255, 0, 0]), lut.apply_u8([0, 255, 0]), lut.apply_u8([255; 3]));
    assert!(gray(r) && gray(g) && g[0] > r[0], "green is lighter than red: {r:?} {g:?}");
    assert!(w[0] >= 254, "{w:?}");
    let red = setup(Profile::srgb(), Profile::srgb(), ViewMode::Channel(Channel::Red)).build().unwrap();
    assert_eq!(red.apply_u8([200, 10, 90]), [200, 200, 200]);
    // a: mid gray is neutral, red is lighter, green darker.
    let a = setup(Profile::srgb(), Profile::srgb(), ViewMode::Channel(Channel::A)).build().unwrap();
    let (n, rr, gg) = (a.apply_u8([128; 3])[0], a.apply_u8([255, 0, 0])[0], a.apply_u8([0, 255, 0])[0]);
    assert!((n as i32 - 128).abs() <= 2 && rr > 160 && gg < 90, "{n} {rr} {gg}");
}

/// A Display P3 document proofed on an sRGB "output": vivid P3 colors are
/// out of its gamut and get the warning, grays and sRGB-safe colors not.
#[test]
fn soft_proof_marks_what_the_output_cannot_show() {
    let p = Proof { output: Profile::srgb(), intent: Intent::RelativeColorimetric, paper_white: false, gamut_warning: Some([0, 255, 255]) };
    let s = setup(Profile::display_p3(), Profile::srgb(), ViewMode::Proof(p.clone()));
    let lut = s.build().unwrap();
    assert_eq!(lut.size, PROOF_LUT_SIZE);
    assert_eq!(lut.apply_u8([255, 0, 0]), [0, 255, 255]);
    assert!(gray(lut.apply_u8([128; 3])) && lut.apply_u8([128; 3]) != [0, 255, 255]);
    assert_ne!(lut.apply_u8([150, 120, 110]), [0, 255, 255]);
    // A gray output shows grays only.
    let g = moxcms::ColorProfile::new_gray_with_gamma(2.2).encode().unwrap();
    let gp = Proof { output: Profile::from_icc(&g, "gray").unwrap(), gamut_warning: None, ..p };
    assert_eq!(gp.output.model(), Model::Gray);
    let lut = setup(Profile::srgb(), Profile::srgb(), ViewMode::Proof(gp)).build().unwrap();
    assert!(gray(lut.apply_u8([200, 30, 90])));
    // Keys change with what the table depends on.
    let k = s.key();
    assert_eq!(k, s.clone().key());
    assert_ne!(k, DisplaySetup { intent: Intent::Saturation, ..s }.key());
}

#[test]
fn buffers_convert_and_profiles_round_trip() {
    let mut px = vec![[255u8, 0, 0, 77], [128, 128, 128, 255]];
    convert_buffer(&Profile::srgb(), &Profile::display_p3(), Intent::RelativeColorimetric, &mut px).unwrap();
    assert!(px[0][0] > 220 && px[0][1] > 30 && px[0][3] == 77, "{:?}", px[0]);
    assert!(gray([px[1][0], px[1][1], px[1][2]]));
    let mut same = vec![[1u8, 2, 3, 4]];
    convert_buffer(&Profile::srgb(), &Profile::srgb(), Intent::Perceptual, &mut same).unwrap();
    assert_eq!(same, vec![[1, 2, 3, 4]]);
    let mut w = ByteWriter::default();
    Profile::display_p3().write(&mut w);
    let b = w.finish();
    assert_eq!(Profile::read(&mut ByteReader::new(&b)).unwrap(), Profile::display_p3());
    assert!(Profile::from_icc(b"not an icc profile", "x").is_err());
    assert_eq!(Intent::from_code(Intent::Saturation.code()), Some(Intent::Saturation));
}

#[test]
fn monitor_choice_falls_back_to_srgb() {
    assert!(monitor_profile(&MonitorChoice::Srgb, None).0.is_srgb());
    let (p, e) = monitor_profile(&MonitorChoice::File("/nowhere/x.icc".into()), None);
    assert!(p.is_srgb() && e.is_some());
}
