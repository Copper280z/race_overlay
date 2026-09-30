//! Shared offscreen video projection for Analysis, Overlay and export.
mod gpu;
mod stitching;
pub use gpu::GpuProjector;
use overlay_core::{DualLensCalibration, Quaternion, VideoProcessingConfig};
pub use stitching::AdvancedStitchingBackend;

pub struct LensImages<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: [&'a [u8]; 2],
    pub timestamp: f64,
}
pub struct VideoProjector {
    gpu: Option<GpuProjector>,
    pub gpu_error: Option<String>,
    advanced: Option<Box<dyn AdvancedStitchingBackend>>,
    prepared_advanced_frame: Option<(f64, u32, u32)>,
}
impl Default for VideoProjector {
    fn default() -> Self {
        Self::new()
    }
}
impl VideoProjector {
    pub fn new() -> Self {
        match GpuProjector::new() {
            Ok(gpu) => Self {
                gpu: Some(gpu),
                gpu_error: None,
                advanced: None,
                prepared_advanced_frame: None,
            },
            Err(e) => Self {
                gpu: None,
                gpu_error: Some(e),
                advanced: None,
                prepared_advanced_frame: None,
            },
        }
    }
    /// Install a backend that has passed the live-preview release gate.
    pub fn set_advanced_backend(&mut self, backend: Box<dyn AdvancedStitchingBackend>) {
        self.advanced = Some(backend);
        self.prepared_advanced_frame = None;
    }
    pub fn backend(&self) -> &str {
        if self.gpu.is_some() { "wgpu" } else { "CPU" }
    }
    pub fn render(
        &mut self,
        images: &LensImages<'_>,
        calibration: &DualLensCalibration,
        config: &VideoProcessingConfig,
        correction: Quaternion,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        validate(images, config, size)?;
        if config.seam == "adaptive" {
            let backend = self.advanced.as_mut().ok_or(
                "Advanced stitching is not available in this build; select Hard cut or Feather",
            )?;
            let key = (images.timestamp, images.width, images.height);
            if self.prepared_advanced_frame != Some(key) {
                backend.prepare(images, calibration)?;
                self.prepared_advanced_frame = Some(key);
            }
            let result = backend.render(config, correction, size)?;
            if result.len() != size.0 as usize * size.1 as usize * 4 {
                return Err("Advanced stitcher returned an incomplete image".into());
            }
            return Ok(result);
        }
        if let Some(gpu) = &mut self.gpu {
            return gpu.render(images, calibration, config, correction, size);
        }
        render_cpu(images, calibration, config, correction, size)
    }
}
fn validate(
    images: &LensImages<'_>,
    config: &VideoProcessingConfig,
    size: (u32, u32),
) -> Result<(), String> {
    config.validate()?;
    if images.width == 0
        || images.height == 0
        || size.0 == 0
        || size.1 == 0
        || u64::from(size.0) * u64::from(size.1) > 16_000_000
        || images
            .pixels
            .iter()
            .any(|p| p.len() as u64 != u64::from(images.width) * u64::from(images.height) * 4)
    {
        return Err("Invalid video frame geometry".into());
    }
    Ok(())
}
fn sample(images: &LensImages<'_>, lens: usize, uv: [f64; 2]) -> [f64; 3] {
    let x = (uv[0] * images.width as f64 - 0.5).clamp(0., images.width as f64 - 1.);
    let y = (uv[1] * images.height as f64 - 0.5).clamp(0., images.height as f64 - 1.);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(images.width - 1);
    let y1 = (y0 + 1).min(images.height - 1);
    let at = |x, y, k| images.pixels[lens][((y * images.width + x) * 4) as usize + k] as f64;
    std::array::from_fn(|k| {
        let a = at(x0, y0, k) * (1. - x.fract()) + at(x1, y0, k) * x.fract();
        let b = at(x0, y1, k) * (1. - x.fract()) + at(x1, y1, k) * x.fract();
        a * (1. - y.fract()) + b * y.fract()
    })
}
pub fn render_cpu(
    images: &LensImages<'_>,
    calibration: &DualLensCalibration,
    config: &VideoProcessingConfig,
    correction: Quaternion,
    size: (u32, u32),
) -> Result<Vec<u8>, String> {
    validate(images, config, size)?;
    if config.seam == "adaptive" {
        return Err("The CPU reference supports Hard cut and Feather; advanced stitching requires a registered backend".into());
    }
    let (width, height) = size;
    let rotation = correction.multiply(config.view.rotation());
    let scale = 2. * (config.horizontal_fov_degrees.to_radians() * 0.5).tan() / width as f64;
    let feather = if config.seam == "feather" {
        2. * config.feather_degrees.to_radians()
    } else {
        0.
    };
    let mut out = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let ray = rotation.rotate([
                (x as f64 + 0.5 - width as f64 * 0.5) * scale,
                (y as f64 + 0.5 - height as f64 * 0.5) * scale,
                1.,
            ]);
            let values = calibration.lenses.each_ref().map(|l| {
                l.project(ray)
                    .filter(|(uv, _)| (uv[0] - 0.5).powi(2) + (uv[1] - 0.5).powi(2) <= 0.25)
            });
            let color = match values {
                [Some((uv0, a)), Some((uv1, b))] => {
                    let w = if feather <= 0. {
                        if a <= b { 1. } else { 0. }
                    } else {
                        let t = ((b - a) / feather + 0.5).clamp(0., 1.);
                        let weight = t * t * (3. - 2. * t);
                        let coverage = |uv: [f64; 2], angle: f64, lens: usize| {
                            (calibration.lenses[lens].max_angle - angle)
                                .min(
                                    (0.5 - ((uv[0] - 0.5).powi(2) + (uv[1] - 0.5).powi(2)).sqrt())
                                        * std::f64::consts::PI,
                                )
                                .max(0.)
                        };
                        let front = weight * coverage(uv0, a, 0);
                        let rear = (1. - weight) * coverage(uv1, b, 1);
                        if front + rear > 1e-12 {
                            front / (front + rear)
                        } else {
                            weight
                        }
                    };
                    let c0 = sample(images, 0, uv0);
                    let c1 = sample(images, 1, uv1);
                    std::array::from_fn(|i| c0[i] * w + c1[i] * (1. - w))
                }
                [Some((uv, _)), None] => sample(images, 0, uv),
                [None, Some((uv, _))] => sample(images, 1, uv),
                _ => [0.; 3],
            };
            let start = ((y * width + x) * 4) as usize;
            for k in 0..3 {
                out[start + k] = color[k].round().clamp(0., 255.) as u8;
            }
            out[start + 3] = 255;
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use overlay_core::LensCalibration;
    #[test]
    fn advanced_preparation_is_cached_independently_of_the_view() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Backend(Arc<AtomicUsize>);
        impl AdvancedStitchingBackend for Backend {
            fn name(&self) -> &str {
                "test"
            }
            fn prepare(
                &mut self,
                _: &LensImages<'_>,
                _: &DualLensCalibration,
            ) -> Result<(), String> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            fn render(
                &mut self,
                _: &VideoProcessingConfig,
                _: Quaternion,
                size: (u32, u32),
            ) -> Result<Vec<u8>, String> {
                Ok(vec![0; size.0 as usize * size.1 as usize * 4])
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let mut p = VideoProjector {
            gpu: None,
            gpu_error: None,
            advanced: None,
            prepared_advanced_frame: None,
        };
        p.set_advanced_backend(Box::new(Backend(count.clone())));
        let pixels = vec![0; 8 * 8 * 4];
        let mut images = LensImages {
            width: 8,
            height: 8,
            pixels: [&pixels, &pixels],
            timestamp: 0.,
        };
        let cal = calibration();
        let mut config = VideoProcessingConfig {
            seam: "adaptive".into(),
            ..Default::default()
        };
        p.render(&images, &cal, &config, Quaternion::IDENTITY, (8, 8))
            .unwrap();
        config.view.yaw = 90.;
        config.horizontal_fov_degrees = 45.;
        p.render(&images, &cal, &config, Quaternion::IDENTITY, (16, 8))
            .unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        images.timestamp = 1.;
        p.render(&images, &cal, &config, Quaternion::IDENTITY, (8, 8))
            .unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
    fn calibration() -> DualLensCalibration {
        let l = LensCalibration {
            xi: 1.,
            focal: [0.4; 2],
            center: [0.5; 2],
            distortion: [0.; 5],
            rig_to_lens: Quaternion::IDENTITY,
            translation: [0.; 3],
            max_angle: 100_f64.to_radians(),
        };
        DualLensCalibration {
            lenses: [
                l.clone(),
                LensCalibration {
                    rig_to_lens: Quaternion::axis_angle([0., 1., 0.], std::f64::consts::PI),
                    ..l
                },
            ],
        }
    }
    #[test]
    fn hard_cut_does_not_mix_colors_and_feather_does() {
        let a = [255, 0, 0, 255].repeat(64);
        let b = [0, 0, 255, 255].repeat(64);
        let imgs = LensImages {
            width: 8,
            height: 8,
            pixels: [&a, &b],
            timestamp: 0.,
        };
        let mut c = VideoProcessingConfig::default();
        c.view.yaw = 90.;
        let hard = render_cpu(&imgs, &calibration(), &c, Quaternion::IDENTITY, (101, 51)).unwrap();
        assert!(
            hard.as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0] == 0 || p[2] == 0)
        );
        c.seam = "feather".into();
        c.feather_degrees = 10.;
        let soft = render_cpu(&imgs, &calibration(), &c, Quaternion::IDENTITY, (101, 51)).unwrap();
        assert!(soft.as_chunks::<4>().0.iter().any(|p| p[0] > 0 && p[2] > 0));
    }
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_matches_cpu() {
        let a = (0..64 * 64 * 4)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        let b = [30, 160, 230, 255].repeat(64 * 64);
        let imgs = LensImages {
            width: 64,
            height: 64,
            pixels: [&a, &b],
            timestamp: 0.,
        };
        let mut gpu = GpuProjector::new().unwrap();
        let mut config = VideoProcessingConfig::default();
        config.view.yaw = 90.;
        config.seam = "feather".into();
        let cpu = render_cpu(
            &imgs,
            &calibration(),
            &config,
            Quaternion::IDENTITY,
            (96, 54),
        )
        .unwrap();
        let output = gpu
            .render(
                &imgs,
                &calibration(),
                &config,
                Quaternion::IDENTITY,
                (96, 54),
            )
            .unwrap();
        assert!(cpu.iter().zip(output).all(|(a, b)| a.abs_diff(b) <= 2));
    }
}
