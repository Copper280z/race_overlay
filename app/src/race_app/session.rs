//! Overlay document/session ownership.

use overlay_core::{
    ProjectDocument, SourceConfig, SourceId, TelemetryDataset, WidgetConfig, WidgetId,
};
use overlay_media::VideoMetadata;
use std::path::PathBuf;

/// Persistent overlay document state. Keeping project/data ownership in one
/// value makes feature modules explicit about the session they mutate and
/// keeps workspace/application concerns out of source and widget policies.
pub(super) struct OverlaySession {
    project: Option<ProjectDocument>,
    project_path: Option<PathBuf>,
    metadata: Option<VideoMetadata>,
    datasets: Vec<TelemetryDataset>,
    current_time: f64,
}

impl OverlaySession {
    pub(super) fn new() -> Self {
        Self {
            project: None,
            project_path: None,
            metadata: None,
            datasets: Vec::new(),
            current_time: 0.0,
        }
    }

    pub(super) fn project(&self) -> Option<&ProjectDocument> {
        self.project.as_ref()
    }
    pub(super) fn project_mut(&mut self) -> Option<&mut ProjectDocument> {
        self.project.as_mut()
    }
    pub(super) fn set_project(&mut self, project: ProjectDocument) {
        self.project = Some(project);
    }
    pub(super) fn has_project(&self) -> bool {
        self.project.is_some()
    }
    pub(super) fn set_project_path(&mut self, path: Option<PathBuf>) {
        self.project_path = path;
    }
    pub(super) fn project_path(&self) -> Option<&PathBuf> {
        self.project_path.as_ref()
    }
    pub(super) fn source(&self, id: SourceId) -> Option<&SourceConfig> {
        self.project()?
            .v1()
            .sources
            .iter()
            .find(|source| source.id == id)
    }
    pub(super) fn source_mut(&mut self, id: SourceId) -> Option<&mut SourceConfig> {
        let ProjectDocument::V1(project) = self.project_mut()?;
        project.sources.iter_mut().find(|source| source.id == id)
    }
    pub(super) fn widget(&self, id: WidgetId) -> Option<&WidgetConfig> {
        self.project()?
            .v1()
            .widgets
            .iter()
            .find(|widget| widget.id == id)
    }
    pub(super) fn widget_mut(&mut self, id: WidgetId) -> Option<&mut WidgetConfig> {
        let ProjectDocument::V1(project) = self.project_mut()?;
        project.widgets.iter_mut().find(|widget| widget.id == id)
    }
    pub(super) fn remove_source(&mut self, id: SourceId) -> Option<usize> {
        let ProjectDocument::V1(project) = self.project_mut()?;
        let index = project.sources.iter().position(|source| source.id == id)?;
        project.sources.remove(index);
        for widget in &mut project.widgets {
            widget
                .bindings
                .retain(|binding| binding.channel.source_id != id);
        }
        self.datasets.retain(|dataset| dataset.source_id != id);
        Some(index)
    }
    pub(super) fn metadata(&self) -> Option<&VideoMetadata> {
        self.metadata.as_ref()
    }
    pub(super) fn set_metadata(&mut self, metadata: VideoMetadata) {
        self.metadata = Some(metadata);
    }
    pub(super) fn current_time(&self) -> f64 {
        self.current_time
    }
    pub(super) fn set_current_time(&mut self, time: f64) {
        self.current_time = time;
    }
    pub(super) fn datasets(&self) -> &[TelemetryDataset] {
        &self.datasets
    }
    pub(super) fn datasets_mut(&mut self) -> &mut Vec<TelemetryDataset> {
        &mut self.datasets
    }
    pub(super) fn clear_datasets(&mut self) {
        self.datasets.clear();
    }
}
