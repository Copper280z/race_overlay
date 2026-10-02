use super::*;

pub(super) fn replace_segments_preserving_identity(
    recording: &mut Recording,
    mut segments: Vec<RunSegment>,
) {
    let mut available = (0..recording.segments.len()).collect::<Vec<_>>();
    for segment in &mut segments {
        let best = available.iter().enumerate().max_by(|(_, a), (_, b)| {
            let score = |index: usize| {
                let previous = &recording.segments[index];
                let overlap = (segment.end_recording_time.min(previous.end_recording_time)
                    - segment
                        .start_recording_time
                        .max(previous.start_recording_time))
                .max(0.0);
                let boundary_error = (segment.start_recording_time - previous.start_recording_time)
                    .abs()
                    + (segment.end_recording_time - previous.end_recording_time).abs();
                (overlap > 0.0, overlap, -boundary_error)
            };
            let left = score(**a);
            let right = score(**b);
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.total_cmp(&right.1))
                .then_with(|| left.2.total_cmp(&right.2))
        });
        if let Some((available_index, previous_index)) = best {
            segment.id = recording.segments[*previous_index].id;
            available.swap_remove(available_index);
        }
    }
    recording.segments = segments;
}

impl AnalysisApp {
    /// Replaces each recording's intervals with laps between crossings of the
    /// single circuit gate. Point-to-point gates never trim intervals; they
    /// only align runs (see `prepare_comparison`).
    pub(super) fn split_laps_at_gate(&mut self) {
        let [gate] = self.workspace.course.gates[..] else {
            return;
        };
        let mut matched = 0;
        let mut unmatched = 0;
        for r in &mut self.workspace.recordings {
            let Some(data) = self.data.get(&r.primary_source) else {
                unmatched += 1;
                continue;
            };
            let pairs = gate_laps(&gps_points(&data.raw, r), gate);
            if pairs.is_empty() {
                unmatched += 1;
                continue;
            }
            matched += 1;
            let segments = pairs
                .into_iter()
                .enumerate()
                .map(|(i, (start, end))| RunSegment {
                    id: SegmentId::new(),
                    name: format!("GPS lap {}", i + 1),
                    start_recording_time: start,
                    end_recording_time: end,
                    kind: SegmentKind::Lap,
                    estimated: true,
                    competitive: true,
                    unknown: Default::default(),
                })
                .collect();
            replace_segments_preserving_identity(r, segments);
        }
        self.retain_valid_selection();
        self.state.cursor = 0.0;
        self.state.range = None;
        self.message = format!(
            "Split laps in {matched} recording(s); {unmatched} without two gate crossings kept their intervals."
        );
    }
    pub(super) fn restore_detected_intervals(&mut self) {
        for recording in &mut self.workspace.recordings {
            let Some(data) = self.data.get(&recording.primary_source) else {
                continue;
            };
            let gps = gps_points(&data.raw, recording);
            let detected = auto_segments(&data.raw, recording, &gps);
            replace_segments_preserving_identity(recording, detected);
        }
        self.retain_valid_selection();
        self.state.cursor = 0.0;
        self.state.range = None;
        self.message = "Restored logger-lap or automatically detected intervals; the analysis range was cleared.".into();
    }
    /// The gate under the reference run's playhead plus `nudge_seconds`. The
    /// nudge reaches past the run's own ends, which the playhead cannot.
    pub(super) fn reference_gate_capture(
        &self,
        nudge_seconds: f64,
    ) -> Option<(RecordingId, String, f64, Gate)> {
        let key = self.workspace.reference.as_ref()?;
        let (recording, interval) = segment(&self.workspace, key)?;
        let data = self.data.get(&recording.primary_source)?;
        let gps = gps_points(&data.raw, recording);
        let time = interval.start_recording_time + self.state.cursor + nudge_seconds;
        let index = gps.partition_point(|point| point.recording_time < time);
        let point = gps.get(index)?;
        // Never place a gate from GPS that is not near the requested moment.
        if (point.recording_time - time).abs() > 2.0 {
            return None;
        }
        let (before, after) = if let Some(after) = gps.get(index + 5) {
            (point, after)
        } else {
            (gps.get(index.saturating_sub(5))?, point)
        };
        let dy = after.latitude - before.latitude;
        let dx = (after.longitude - before.longitude) * point.latitude.to_radians().cos();
        Some((
            recording.id,
            format!("{} / {}", recording.name, interval.name),
            point.recording_time,
            Gate {
                latitude: point.latitude,
                longitude: point.longitude,
                heading_degrees: dx.atan2(dy).to_degrees(),
                width_meters: 12.0,
            },
        ))
    }
    pub(super) fn open_path(&mut self, path: PathBuf) {
        match AnalysisDocument::load(&path) {
            Ok(doc) => {
                self.pause();
                self.initial_layout_pending = false;
                self.workspace = doc.workspace().clone();
                self.state = serde_json::from_value(
                    self.workspace
                        .settings
                        .get("ui")
                        .cloned()
                        .unwrap_or(Value::Null),
                )
                .unwrap_or_default();
                self.dock = serde_json::from_value(self.state.dock.clone())
                    .unwrap_or_else(|_| DockState::new(vec![]));
                if self.dock.iter_all_tabs().next().is_none() {
                    let has_video = self
                        .workspace
                        .recordings
                        .iter()
                        .any(|r| r.video_path.is_some());
                    self.layout(if has_video { 0 } else { 1 });
                }
                self.next_tab = self
                    .dock
                    .iter_all_tabs()
                    .map(|(_, t)| t.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                self.path = Some(path);
                self.auto_select_pending = false;
                self.auto_mode_pending = false;
                self.automatic_mode = false;
                self.data.clear();
                self.loading.clear();
                self.video_sync.clear();
                self.maps.clear();
                self.errors.clear();
                self.active_overlay = None;
                let sources = self
                    .workspace
                    .recordings
                    .iter()
                    .flat_map(|r| r.sources.clone())
                    .collect::<Vec<_>>();
                for source in sources {
                    self.enqueue(source);
                }
                self.changed();
                self.dirty = false;
            }
            Err(analysis_error) => match ProjectDocument::load(&path) {
                Ok(doc) => self.import_project(doc.v1().clone()),
                Err(_) => self.errors.push(analysis_error.to_string()),
            },
        }
    }
    pub(super) fn save(&mut self, as_new: bool) {
        let path = if as_new { None } else { self.path.clone() }.or_else(|| {
            rfd::FileDialog::new()
                .set_file_name("session.race-analysis.json")
                .save_file()
        });
        let Some(path) = path else {
            return;
        };
        self.state.dock = serde_json::to_value(&self.dock).unwrap_or(Value::Null);
        if !self.workspace.settings.is_object() {
            self.workspace.settings = json!({});
        }
        self.workspace.settings["ui"] = serde_json::to_value(&self.state).unwrap_or(Value::Null);
        match AnalysisDocument::V1(self.workspace.clone()).save_atomic(&path) {
            Ok(()) => {
                self.path = Some(path);
                self.dirty = false;
                self.message = "Workspace saved; recordings remain external files.".into();
            }
            Err(e) => self.errors.push(e.to_string()),
        }
    }
}
