//! Export job preparation and execution.

use super::super::policy::export_surface;
use super::{OverlayEditor, WorkerEvent};
use overlay_media::{CancelToken, ExportSettings, RgbaFrame, export_video};
use overlay_render::{
    AlignedDatasets, RenderOptions, RenderSize, prepare_project_widgets_with_appearance,
    render_prepared_project_widgets,
};
use std::{collections::HashMap, thread};

impl OverlayEditor {
    pub(super) fn export_with_settings(&mut self, settings: ExportSettings) {
        if self.export_controller.export_cancel.is_some() {
            return;
        }
        let (Some(project), Some(metadata), Some(tools)) = (
            self.project().cloned(),
            self.session.metadata().cloned(),
            self.tools().cloned(),
        ) else {
            self.status = "Open a video and configure FFmpeg before exporting".into();
            return;
        };
        let Some(output) = rfd::FileDialog::new()
            .add_filter("MPEG-4 video", &["mp4"])
            .set_file_name("race-overlay.mp4")
            .save_file()
        else {
            return;
        };
        let datasets = self.session.datasets().to_vec();
        let offsets: HashMap<_, _> = project
            .sources
            .iter()
            .map(|source| (source.id, source.alignment.offset_seconds))
            .collect();
        let tx = self.worker_hub.sender();
        let cancel = CancelToken::new();
        self.export_controller.export_cancel = Some(cancel.clone());
        self.status = "Exporting overlay…".into();
        thread::spawn(move || {
            let size = RenderSize::new(metadata.width, metadata.height);
            let aligned = AlignedDatasets {
                datasets: &datasets,
                source_offsets: offsets,
                cache_filtered_series: true,
                ..Default::default()
            };
            let Some((geometry, export_widgets)) = export_surface(&project.widgets, size) else {
                let _ = tx.send(WorkerEvent::ExportFinished(Err(
                    "There are no visible widgets to export".into(),
                )));
                return;
            };
            // Static widget geometry is prepared once rather than per frame.
            let prepared_widgets = prepare_project_widgets_with_appearance(
                &export_widgets,
                &aligned,
                &project.appearance,
            );
            let fps = metadata.fps().unwrap_or(30.0);
            let frame_count = (metadata.duration.unwrap_or(0.0) * fps).ceil() as u64;
            let mut frame = 0u64;
            let mut provider = || -> Result<Option<RgbaFrame>, overlay_media::MediaError> {
                if frame >= frame_count || cancel.is_cancelled() {
                    return Ok(None);
                }
                let t = frame as f64 / fps;
                let rendered = render_prepared_project_widgets(
                    &prepared_widgets,
                    &aligned,
                    RenderSize::new(geometry.width, geometry.height),
                    t,
                    RenderOptions {
                        crop: false,
                        full_size: false,
                    },
                );
                frame += 1;
                RgbaFrame::new(
                    rendered.image.width,
                    rendered.image.height,
                    rendered.image.pixels,
                )
                .map(Some)
            };
            let result = export_video(
                &tools,
                &project.video_path,
                &output,
                &settings,
                geometry,
                &mut provider,
                &cancel,
                |progress| {
                    let _ = tx.send(WorkerEvent::ExportProgress(progress));
                },
            )
            .map(|_| output.clone())
            .map_err(|error| error.to_string());
            let _ = tx.send(WorkerEvent::ExportFinished(result));
        });
    }
}
