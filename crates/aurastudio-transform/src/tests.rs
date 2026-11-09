use aurastudio_core::{Point, Rect, Rgba8};

use crate::*;

fn checker(w: u32, h: u32) -> Vec<Rgba8> {
    (0..w * h).map(|i| { let (x, y) = (i % w, i / w); if (x / 4 + y / 4) % 2 == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] } }).collect()
}

fn pt(x: f64, y: f64) -> Point { Point::new(x, y) }

#[test]
fn homographies_map_corners_and_invert() {
    let q = [pt(10.0, 5.0), pt(90.0, 20.0), pt(80.0, 70.0), pt(5.0, 60.0)];
    let h = Homography::rect_to_quad(Rect::new(0, 0, 40, 30), q).unwrap();
    for (c, e) in corners(Rect::new(0, 0, 40, 30)).iter().zip(q) {
        assert!(h.apply(*c).distance(e) < 1e-6);
    }
    let i = h.inverse().unwrap();
    assert!(h.apply(i.apply(pt(50.0, 40.0))).distance(pt(50.0, 40.0)) < 1e-6);
    assert!(Homography::square_to_quad([pt(0.0, 0.0), pt(1.0, 0.0), pt(2.0, 0.0), pt(3.0, 0.0)]).is_none());
    assert!(convex(&q) && !convex(&[pt(0.0, 0.0), pt(10.0, 0.0), pt(2.0, 2.0), pt(0.0, 10.0)]));
    let k = keep_convex(q, [pt(60.0, 50.0), q[1], q[2], q[3]]);
    assert!(convex(&k));
}

#[test]
fn identity_and_moves_keep_the_pixels() {
    let (w, h) = (32, 24);
    let px = checker(w as u32, h as u32);
    let src = Source::new(Rect::new(8, 8, 8 + w, 8 + h), &px);
    for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
        // Moved by whole pixels (negative coordinates too): the same pixels.
        for (x, y) in [(20, 10), (-50, -7)] {
            let to = Rect::new(x, y, x + w, y + h);
            let warp = Warp { quad: corners(to), mesh: None };
            let mut out = vec![[0u8; 4]; (w * h) as usize];
            render(&src, &warp, interp, to, &mut out);
            assert_eq!(out, px, "{interp:?}");
        }
    }
    // A grid left as it is changes nothing either.
    let warp = Warp { quad: corners(Rect::new(8, 8, 8 + w, 8 + h)), mesh: Some(Mesh::new(3, 3)) };
    let mut out = vec![[0u8; 4]; (w * h) as usize];
    render(&src, &warp, Interp::Bicubic, Rect::new(8, 8, 8 + w, 8 + h), &mut out);
    assert_eq!(out, px);
}

#[test]
fn halving_averages_and_rotating_keeps_alpha() {
    // A 1 px checker shrunk to half: an even mix, not aliasing.
    let fine: Vec<Rgba8> = (0..64 * 64).map(|i| if (i % 64 + i / 64) % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
    let src = Source::new(Rect::new(0, 0, 64, 64), &fine);
    let warp = Warp { quad: corners(Rect::new(0, 0, 32, 32)), mesh: None };
    let mut out = vec![[0u8; 4]; 32 * 32];
    render(&src, &warp, Interp::Bicubic, Rect::new(0, 0, 32, 32), &mut out);
    for p in &out[10 * 32 + 10..10 * 32 + 20] { assert!((110..=145).contains(&p[0]), "{p:?}"); }
    assert!(src.heap_bytes() > 64 * 64 * 4, "the halving was made when needed");
    // A square turned 45°: its middle opaque, its corners outside empty,
    // soft edges.
    let sq = vec![[200u8, 50, 10, 255]; 40 * 40];
    let src = Source::new(Rect::new(30, 30, 70, 70), &sq);
    let (c, r) = (50.0, 20.0 * 2f64.sqrt());
    let warp = Warp { quad: [pt(c, c - r), pt(c + r, c), pt(c, c + r), pt(c - r, c)], mesh: None };
    let dr = warp.dest_rect(Rect::new(30, 30, 70, 70)).unwrap();
    let mut out = vec![[0u8; 4]; dr.area() as usize];
    render(&src, &warp, Interp::Bicubic, dr, &mut out);
    let px = |x: i32, y: i32| out[((y - dr.y0) * dr.w() as i32 + x - dr.x0) as usize];
    assert_eq!(px(50, 50), [200, 50, 10, 255]);
    assert_eq!(px(dr.x0, dr.y0)[3], 0);
    assert!(out.iter().any(|p| p[3] > 0 && p[3] < 255));
    assert!(out.iter().all(|p| p[3] == 0 || p[0] >= 190));
}

/// A point moves its four cells only: the points around and what is past
/// them stay.
#[test]
fn a_warp_point_moves_its_cells_only() {
    let mut m = Mesh::new(5, 5);
    let k = 2 * 6 + 2;
    m.pts[k] = m.pts[k] + pt(0.07, -0.05);
    for j in 0..=5 {
        for i in 0..=5 {
            let u = pt(i as f64 / 5.0, j as f64 / 5.0);
            let e = m.eval(u);
            if (i, j) == (2, 2) { assert!(e.distance(u + pt(0.07, -0.05)) < 1e-12); } else { assert!(e.distance(u) < 1e-12, "point {i},{j} moved"); }
        }
    }
    assert!((m.eval(pt(0.35, 0.45)).x - 0.35).abs() > 1e-3);
    for (u, v) in [(0.15, 0.4), (0.65, 0.4), (0.4, 0.1), (0.4, 0.7), (0.9, 0.9), (0.62, 0.62)] {
        assert_eq!(m.eval(pt(u, v)), pt(u, v), "{u},{v} moved");
    }
    // The whole grid moved: the whole image moves the same.
    let mut all = Mesh::new(3, 3);
    for p in &mut all.pts { p.x += 0.1; }
    for (u, v) in [(0.0, 0.0), (0.33, 0.71), (1.0, 0.5)] { assert!(all.eval(pt(u, v)).distance(pt(u + 0.1, v)) < 1e-12); }
}

#[test]
fn a_bent_grid_moves_its_middle() {
    let px = vec![[255u8; 4]; 40 * 40];
    let r = Rect::new(0, 0, 40, 40);
    let src = Source::new(r, &px);
    let mut m = Mesh::new(2, 2);
    m.pts[4] = pt(0.5, 0.25);
    let warp = Warp { quad: corners(r), mesh: Some(m.clone()) };
    // The middle of the source now lands at a quarter.
    let p = warp.map(r, pt(20.0, 20.0)).unwrap();
    assert!(p.distance(pt(20.0, 10.0)) < 1e-6, "{p:?}");
    assert_eq!(m.eval(pt(0.0, 0.0)), pt(0.0, 0.0));
    assert_eq!(m.eval(pt(1.0, 1.0)), pt(1.0, 1.0));
    let mut out = vec![[0u8; 4]; 40 * 40];
    render(&src, &warp, Interp::Bilinear, r, &mut out);
    assert_eq!(out[38 * 40 + 38][3], 255);
    assert_eq!(out[20 * 40 + 20][3], 255);
    assert_eq!(out[2 * 40 + 2][3], 255);
}

/// Rendering a rect in pieces gives what rendering it at once gives
/// (bands, blocks and the lazy pyramid hold no state between calls).
#[test]
fn pieces_match_the_whole() {
    let (w, h) = (90u32, 70u32);
    let px: Vec<Rgba8> = (0..w * h).map(|i| [(i % 251) as u8, (i % 113) as u8, (i % 61) as u8, (i % 7 * 40) as u8]).collect();
    let s = Rect::new(0, 0, w as i32, h as i32);
    let src = Source::new(s, &px);
    let mut m = Mesh::new(3, 3);
    m.pts[5] = m.pts[5] + pt(0.05, 0.03);
    for warp in [Warp { quad: [pt(5.0, 3.0), pt(60.0, 10.0), pt(50.0, 40.0), pt(-4.0, 33.0)], mesh: None }, Warp { quad: corners(Rect::new(-10, 0, 40, 30)), mesh: Some(m) }] {
        let d = warp.dest_rect(s).unwrap();
        let mut whole = vec![[0u8; 4]; d.area() as usize];
        render(&src, &warp, Interp::Bicubic, d, &mut whole);
        let mid = d.x0 + d.w() as i32 / 2;
        for part in [Rect::new(d.x0, d.y0, mid, d.y1), Rect::new(mid, d.y0, d.x1, d.y1)] {
            let mut out = vec![[0u8; 4]; part.area() as usize];
            render(&src, &warp, Interp::Bicubic, part, &mut out);
            for y in part.y0..part.y1 {
                for x in part.x0..part.x1 {
                    let a = out[((y - part.y0) * part.w() as i32 + x - part.x0) as usize];
                    let b = whole[((y - d.y0) * d.w() as i32 + x - d.x0) as usize];
                    // (A block's scale is taken at its center: the pieces cut
                    // blocks elsewhere, a level may differ by one at a seam.)
                    assert!(a.iter().zip(b).all(|(p, q)| (*p as i32 - q as i32).abs() <= 2), "{x},{y}: {a:?} {b:?}");
                }
            }
        }
    }
}
