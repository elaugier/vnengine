#[cfg(test)]
mod tests {
    use crate::renderer::{full_screen_quad, quad_vertices_color, quad_vertices_textured, UiBox};

    #[test]
    fn full_screen_quad_wide_image() {
        // 16:9 image on 16:9 screen: should fill entirely.
        let q = full_screen_quad(1920, 1080, 1.0);
        assert!((q.hw - 1.0).abs() < 0.01);
        assert!((q.hh - 1.0).abs() < 0.01);
        assert_eq!(q.color, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn full_screen_quad_tall_image() {
        // 9:16 image on 16:9 screen: should be letterboxed (hw < 1).
        let q = full_screen_quad(720, 1280, 0.5);
        assert!(q.hw < 1.0);
        assert!((q.hh - 1.0).abs() < 0.01);
        assert!((q.color[3] - 0.5).abs() < 0.01);
    }

    #[test]
    fn full_screen_quad_wide_image_letterbox() {
        // 21:9 image on 16:9 screen: should be pillarboxed (hh < 1).
        let q = full_screen_quad(2520, 1080, 1.0);
        assert!((q.hw - 1.0).abs() < 0.01);
        assert!(q.hh < 1.0);
    }

    #[test]
    fn full_screen_quad_centered() {
        let q = full_screen_quad(100, 100, 1.0);
        assert!((q.x - 0.5).abs() < 0.01);
        assert!((q.y - 0.5).abs() < 0.01);
    }

    #[test]
    fn quad_vertices_textured_count() {
        let q = full_screen_quad(1920, 1080, 1.0);
        let verts = quad_vertices_textured(&q);
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn quad_vertices_textured_uv_range() {
        let q = full_screen_quad(1920, 1080, 1.0);
        let verts = quad_vertices_textured(&q);
        for v in &verts {
            assert!(v.uv[0] >= 0.0 && v.uv[0] <= 1.0);
            assert!(v.uv[1] >= 0.0 && v.uv[1] <= 1.0);
        }
    }

    #[test]
    fn quad_vertices_textured_ndc_range() {
        // A full-screen 16:9 quad: hw=1.0, hh=1.0, centered at (0.5, 0.5).
        // NDC = v*2-1, so x in [0.5-1, 0.5+1] = [-0.5, 1.5] -> NDC [-2, 2].
        // The quad intentionally extends beyond the screen for full coverage.
        let q = full_screen_quad(1920, 1080, 1.0);
        let verts = quad_vertices_textured(&q);
        // All vertices should be within reasonable NDC bounds.
        for v in &verts {
            assert!(v.pos[0] >= -2.01 && v.pos[0] <= 2.01);
            assert!(v.pos[1] >= -2.01 && v.pos[1] <= 2.01);
        }
        // Should have vertices at the corners of the quad.
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos[1]).collect();
        assert!(xs.iter().any(|&x| (x - (-2.0)).abs() < 0.01));
        assert!(xs.iter().any(|&x| (x - 2.0).abs() < 0.01));
        assert!(ys.iter().any(|&y| (y - (-2.0)).abs() < 0.01));
        assert!(ys.iter().any(|&y| (y - 2.0).abs() < 0.01));
    }

    #[test]
    fn quad_vertices_color_count() {
        let b = UiBox {
            x: 0.1,
            y: 0.2,
            w: 0.5,
            h: 0.3,
            color: [1.0, 0.0, 0.0, 0.5],
        };
        let verts = quad_vertices_color(&b);
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn quad_vertices_color_ndc() {
        let b = UiBox {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
            color: [0.0, 0.0, 0.0, 1.0],
        };
        let verts = quad_vertices_color(&b);
        // Full screen: corners should be at -1 and +1.
        let xs: Vec<f32> = verts.iter().map(|v| v.pos[0]).collect();
        let ys: Vec<f32> = verts.iter().map(|v| v.pos[1]).collect();
        assert!(xs.iter().any(|&x| (x - (-1.0)).abs() < 0.01));
        assert!(xs.iter().any(|&x| (x - 1.0).abs() < 0.01));
        assert!(ys.iter().any(|&y| (y - (-1.0)).abs() < 0.01));
        assert!(ys.iter().any(|&y| (y - 1.0).abs() < 0.01));
    }

    #[test]
    fn quad_vertices_color_propagates_color() {
        let b = UiBox {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
            color: [0.5, 0.5, 0.5, 0.8],
        };
        let verts = quad_vertices_color(&b);
        for v in &verts {
            assert_eq!(v.color, [0.5, 0.5, 0.5, 0.8]);
        }
    }
}
