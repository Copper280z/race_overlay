//! Extension point for frame-specific stitching in camera coordinates.
use super::{DualLensCalibration, LensImages, Quaternion, VideoProcessingConfig};

/// A backend must prepare correspondence, exposure, and seams independently of the
/// selected view. `render` reuses that preparation when a paused user aims/zooms.
/// Implementations must reproduce a frame after seeking, bound their caches, and
/// report unsupported geometry instead of substituting another stitching policy.
/// Backend registration is reserved for implementations that pass the live gate.
pub trait AdvancedStitchingBackend: Send {
    fn name(&self) -> &str;
    fn prepare(
        &mut self,
        images: &LensImages<'_>,
        calibration: &DualLensCalibration,
    ) -> Result<(), String>;
    fn render(
        &mut self,
        config: &VideoProcessingConfig,
        correction: Quaternion,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String>;
}
