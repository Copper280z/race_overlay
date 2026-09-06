use super::*;

pub(super) fn replace_segments_preserving_identity(
    recording: &mut Recording,
    mut segments: Vec<RunSegment>,
) {
    for (segment, previous) in segments.iter_mut().zip(&recording.segments) {
        segment.id = previous.id;
    }
    recording.segments = segments;
}

fn interval_bar(ui: &mut egui::Ui, start: &mut f64, end: &mut f64, bounds: (f64, f64)) -> bool {
    if !bounds.0.is_finite() || !bounds.1.is_finite() || bounds.1 <= bounds.0 {
        return false;
    }
    ui.small("Drag the endpoints to trim the interval");
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(80.0), 28.0),
        egui::Sense::drag(),
    );
    let x = |t: f64| {
        rect.left() + ((t - bounds.0) / (bounds.1 - bounds.0)).clamp(0.0, 1.0) as f32 * rect.width()
    };
    let stroke = egui::Stroke::new(3.0, ui.visuals().selection.bg_fill);
    ui.painter().line_segment(
        [rect.left_center(), rect.right_center()],
        egui::Stroke::new(2.0, ui.visuals().weak_text_color()),
    );
    ui.painter().line_segment(
        [
            egui::pos2(x(*start), rect.center().y),
            egui::pos2(x(*end), rect.center().y),
        ],
        stroke,
    );
    for t in [*start, *end] {
        ui.painter().circle_filled(
            egui::pos2(x(t), rect.center().y),
            6.0,
            ui.visuals().selection.stroke.color,
        );
    }
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        ui.data_mut(|d| {
            d.insert_temp(
                response.id,
                (p.x - x(*start)).abs() <= (p.x - x(*end)).abs(),
            )
        });
    }
    if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
    {
        let t = bounds.0
            + ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * (bounds.1 - bounds.0);
        if ui
            .data_mut(|d| d.get_temp::<bool>(response.id))
            .unwrap_or(true)
        {
            *start = t.min(*end - 0.001);
        } else {
            *end = t.max(*start + 0.001);
        }
        return true;
    }
    false
}

impl AnalysisApp {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add files…").clicked()
                && let Some(paths) = rfd::FileDialog::new()
                    .add_filter(
                        "Data / video",
                        &["xrk", "csv", "insv", "lrv", "mp4", "mov", "mkv"],
                    )
                    .pick_files()
            {
                self.add_paths(paths);
            }
            if ui.button("Open workspace…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Workspace / overlay project", &["json"])
                    .pick_file()
            {
                self.open_path(path);
            }
            if ui
                .button(if self.dirty { "Save *" } else { "Save" })
                .clicked()
            {
                self.save(false);
            }
            if ui.button("Save as…").clicked() {
                self.save(true);
            }
            ui.menu_button("Panels / layout", |ui| {
                for (label, kind) in [
                    ("Channel plot", TabKind::Plot(Default::default())),
                    (
                        "Delta plot",
                        TabKind::Plot(PlotOptions {
                            delta: true,
                            ..Default::default()
                        }),
                    ),
                    (
                        "Video",
                        TabKind::Video(VideoOptions {
                            slot: 0,
                            segment: None,
                            linked: true,
                            time: 0.0,
                            unknown: Default::default(),
                        }),
                    ),
                    (
                        "Course map",
                        TabKind::Map {
                            channel: "gps_speed".into(),
                            settings: Default::default(),
                        },
                    ),
                    (
                        "GPS imagery",
                        TabKind::Map {
                            channel: "gps_speed".into(),
                            settings: Box::new(MapSettings {
                                actual_gps: true,
                                ..Default::default()
                            }),
                        },
                    ),
                    ("Values / stats", TabKind::Stats),
                    ("Recordings", TabKind::Browser),
                    ("Timing / course", TabKind::Setup),
                ] {
                    if ui.button(label).clicked() {
                        let tab = self.tab(kind);
                        self.dock.push_to_focused_leaf(tab);
                        self.dirty = true;
                        ui.close();
                    }
                }
                ui.separator();
                for (i, label) in ["Quick Compare", "Data Focus", "Video Compare"]
                    .iter()
                    .enumerate()
                {
                    if ui.button(*label).clicked() {
                        self.layout(i);
                        ui.close();
                    }
                }
            });
            ui.separator();
            for mode in [XMode::Time, XMode::Course, XMode::Distance] {
                let label = if mode == XMode::Time
                    && automatic_alignment_complete(
                        &self.prepared,
                        self.workspace.reference.as_ref(),
                    ) {
                    "Aligned time"
                } else {
                    mode.label()
                };
                if ui
                    .selectable_value(&mut self.state.mode, mode, label)
                    .changed()
                {
                    self.automatic_mode = false;
                    self.plot_cache.clear();
                    self.dirty = true;
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(if self.playing { "Pause" } else { "Play linked" })
                .clicked()
            {
                self.playing = !self.playing;
            }
            let duration = self
                .reference_run()
                .map_or(1.0, |r| r.end - r.start)
                .max(0.001);
            ui.add(
                egui::Slider::new(&mut self.state.cursor, 0.0..=duration)
                    .text("Reference elapsed s"),
            );
            if ui.button("Range start").clicked() {
                self.state
                    .range
                    .get_or_insert([self.state.cursor, duration])[0] = self.state.cursor;
            }
            if ui.button("Range finish").clicked() {
                self.state.range.get_or_insert([0.0, self.state.cursor])[1] = self.state.cursor;
            }
            if ui.button("Clear range").clicked() {
                self.state.range = None;
            }
            if self.state.mode == XMode::Course && self.prepared.course.is_none() {
                ui.weak("No reference GPS: showing elapsed time.");
            }
        });
    }
    pub(super) fn browser(&mut self, ui: &mut egui::Ui) {
        let mut changes = false;
        let mut attach = None;
        let mut overlay = None;
        let mut pending_video = None;
        egui::ScrollArea::vertical().show(ui,|ui|{
            if self.workspace.recordings.is_empty(){ui.heading("Start with your data");ui.label("Drop multiple XRK files here. Circuit laps appear automatically; autocross gets an estimated driving interval. No track setup required.");}
            for recording in &mut self.workspace.recordings {
                ui.push_id(recording.id.0,|ui|{
                    let selected=self.state.selected_recording==Some(recording.id);
                    if ui.selectable_label(selected,&recording.name).clicked(){self.state.selected_recording=Some(recording.id);}
                    if selected {changes|=ui.text_edit_singleline(&mut recording.name).changed();ui.horizontal_wrapped(|ui|{
                        if ui.button("Attach video…").clicked(){attach=Some(recording.id);}
                        if ui.add_enabled(recording.video_path.is_some(),egui::Button::new("Edit overlay / sync")).clicked(){overlay=Some(recording.id);}
                        if ui.button("Remove…").clicked(){self.remove_recording=Some(recording.id);}
                    });
                    if let Some(path)=&recording.video_path{ui.small(path.file_name().unwrap_or_default().to_string_lossy());}
                    if let Some(paths)=self.workspace.settings.get("unassigned_videos").and_then(Value::as_array){
                        egui::ComboBox::from_id_salt("unpaired").selected_text("Pair an imported video…").show_ui(ui,|ui|{for v in paths {if let Some(p)=v.as_str() && ui.selectable_label(false,Path::new(p).file_name().unwrap_or_default().to_string_lossy()).clicked(){pending_video=Some((recording.id,PathBuf::from(p)));}}});
                    }}
                    for seg in &recording.segments {
                        let key=SegmentRef{recording_id:recording.id,segment_id:seg.id};let mut checked=self.state.selection.contains(&key);
                        ui.horizontal(|ui|{
                            if ui.checkbox(&mut checked,format!("{} · {:.3}s{}",seg.name,seg.duration(),if seg.estimated{" (estimated)"}else{""})).changed(){if checked{self.state.selection.push(key.clone());}else{self.state.selection.retain(|k|k!=&key);}changes=true;}
                            if ui.selectable_label(self.workspace.reference.as_ref()==Some(&key),"Ref").on_hover_text("Pin this run as the comparison reference").clicked(){self.workspace.reference=Some(key.clone());if !self.state.selection.contains(&key){self.state.selection.push(key);}self.state.cursor=0.0;changes=true;}
                        });
                        if !seg.competitive{ui.weak("Out/in or noncompetitive — available for inspection");}
                        if let Some(run)=self.prepared.runs.iter().find(|r|r.key.recording_id==recording.id && r.key.segment_id==seg.id) {
                            if run.progress.is_empty(){ui.weak("No GPS course match — use elapsed time or traveled distance.");}
                            else {let valid=run.progress.iter().filter(|p|p.confidence>0.0).count();ui.weak(format!("Course match: {:.0}% of GPS samples",valid as f64/run.progress.len() as f64*100.0)).on_hover_text("Matching coverage, not statistical accuracy. Use actual GPS view to inspect the route; anchors can help ambiguous sections.");}
                            if self.prepared.automatic_time_alignment {
                                if self.workspace.reference.as_ref()==Some(&run.key) {ui.weak("Time alignment: reference clock");}
                                else if let Some(alignment)=&run.time_alignment {
                                    let details=match (alignment.distance_offset_meters,alignment.coefficient) {
                                        (Some(distance),Some(coefficient))=>format!("distance shift {distance:+.1}m · start shift {:+.2}s · r {coefficient:+.3}",alignment.offset_seconds),
                                        _=>format!("start shift {:+.2}s",alignment.offset_seconds),
                                    };
                                    ui.weak(format!("Time alignment: {} · {details}",alignment.channel)).on_hover_text("Best-effort comparison shift only; source and video timestamps were not changed.");
                                }
                                else if run.distance.len() >= 2 {ui.colored_label(egui::Color32::LIGHT_RED,"Time alignment unavailable — use traveled distance");}
                                else {ui.colored_label(egui::Color32::LIGHT_RED,"Time alignment unavailable — using segment-relative time");}
                            }
                        }
                    }
                    if recording.segments.is_empty(){ui.weak(if self.loading.contains_key(&recording.primary_source){"Importing…"}else{"No interval. Check source errors or add a range in setup."});}
                    ui.separator();
                });
            }
        });
        if changes {
            self.changed();
        }
        if let Some(id) = attach
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Exported video", &["mp4", "mov", "mkv"])
                .pick_file()
        {
            self.attach_video(id, path);
        }
        if let Some((id, path)) = pending_video {
            self.attach_video(id, path.clone());
            if let Some(paths) = self
                .workspace
                .settings
                .get_mut("unassigned_videos")
                .and_then(Value::as_array_mut)
            {
                paths.retain(|v| v.as_str() != path.to_str());
            }
        }
        if let Some(id) = overlay
            && let Some(project) = self
                .workspace
                .recordings
                .iter()
                .find(|r| r.id == id)
                .map(project_from_recording)
        {
            self.active_overlay = Some(id);
            self.pause();
            self.actions.push(AnalysisAction::OpenOverlay(project));
        }
    }
    pub(super) fn remove_dialog(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.remove_recording {
            egui::Window::new("Remove recording?").collapsible(false).show(ctx,|ui|{
            ui.label("Remove its comparison selections and video attachment? Files on disk will not be deleted.");ui.horizontal(|ui|{
                if ui.button("Remove").clicked(){if let Some(r)=self.workspace.recordings.iter().find(|r|r.id==id){for s in &r.sources{self.data.remove(&s.id);self.loading.remove(&s.id);}}self.workspace.recordings.retain(|r|r.id!=id);self.state.selection.retain(|s|s.recording_id!=id);if self.workspace.reference.as_ref().is_some_and(|r|r.recording_id==id){self.workspace.reference=None;}self.remove_recording=None;self.default_selection();}
                if ui.button("Cancel").clicked(){self.remove_recording=None;}
            });});
        }
    }
    pub(super) fn setup(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut reload = vec![];
        let mut removed_sources = vec![];
        let mut extra = None;
        let mut restore_intervals = false;
        egui::ScrollArea::vertical().show(ui,|ui|{
            ui.label("Intervals use the recording clock, not raw camera time. Video viewers show exported-video time separately.");
            for recording in &mut self.workspace.recordings {
                ui.push_id(recording.id.0,|ui|{ui.collapsing(recording.name.clone(),|ui|{
                    ui.horizontal_wrapped(|ui|{ui.label("Video = recording time +");changed|=ui.add(egui::DragValue::new(&mut recording.video_offset_seconds).speed(0.01).suffix(" s")).changed();});
                    ui.collapsing("Match one visible event",|ui|{
                        ui.small("Unlink a video viewer and find a recognizable event. Enter its exported-video time and the same event's recording time. This sets an offset; it does not warp either run.");
                        let id=ui.id().with("event-pair");let mut pair=ui.data_mut(|d|d.get_temp::<[f64;2]>(id).unwrap_or([0.0,0.0]));
                        ui.add(egui::DragValue::new(&mut pair[0]).speed(0.01).prefix("Video s "));ui.add(egui::DragValue::new(&mut pair[1]).speed(0.01).prefix("Recording s "));
                        if ui.button("Apply matching event").clicked(){recording.video_offset_seconds=pair[0]-pair[1];changed=true;}ui.data_mut(|d|d.insert_temp(id,pair));
                        ui.small("For automatic audio / sensor correlation and camera calibration, use Edit overlay / sync. Those settings return here when you switch back.");
                    });
                    egui::ComboBox::from_id_salt("primary-source").selected_text(recording.sources.iter().find(|s|s.id==recording.primary_source).map_or("Primary data",|s|s.name.as_str())).show_ui(ui,|ui|{for source in &recording.sources{changed|=ui.selectable_value(&mut recording.primary_source,source.id,&source.name).changed();}});
                    if ui.button("Detect intervals from primary data").on_hover_text("Replaces this recording's intervals with logger laps or automatic motion detection.").clicked() && let Some(data)=self.data.get(&recording.primary_source) {
                        let gps=gps_points(&data.raw,recording);let detected=auto_segments(&data.raw,recording,&gps);replace_segments_preserving_identity(recording,detected);changed=true;self.auto_select_pending=true;
                    }
                    for source in &mut recording.sources {ui.push_id(source.id.0,|ui|{
                        ui.label(&source.name);ui.small(source.path.display().to_string());
                        if source.adapter=="generic_csv" {ui.collapsing("CSV columns / units (advanced)",|ui|{
                            ui.small("Default: first row is headers, first column is time in seconds. Configure columns with name, quantity and unit for cross-file comparisons and GPS. Explicit null disables a header/time column.");
                            let id=ui.id().with("csv-json");
                            let mut draft=ui.data_mut(|d|d.get_temp::<String>(id)).unwrap_or_else(||serde_json::to_string_pretty(&source.settings).unwrap_or_default());
                            ui.add(egui::TextEdit::multiline(&mut draft).code_editor().desired_rows(8));
                            if ui.button("Apply CSV settings & reload").clicked(){match serde_json::from_str::<Value>(&draft) {
                                Ok(value) if value.is_object() && serde_json::from_value::<overlay_core::adapters::CsvConfig>(value.clone()).is_ok()=>{source.settings=value;reload.push(source.clone());changed=true;},
                                _=>{self.errors.push("CSV settings must be a valid configuration object; see docs/usage.md.".into());}
                            }}
                            ui.data_mut(|d|d.insert_temp(id,draft));
                        });}
                        changed|=ui.add(egui::DragValue::new(&mut source.alignment.offset_seconds).speed(0.01).prefix("Source minus recording s ")).changed();
                        if !source.settings.is_object(){source.settings=json!({});}
                        let filter_id=ui.id().with("source-filter-draft");
                        let (mut enabled,mut hz)=ui.data_mut(|d|d.get_temp::<(bool,f64)>(filter_id)).unwrap_or((source.settings.get("low_pass_enabled").and_then(Value::as_bool).unwrap_or(false),source.settings.get("low_pass_hz").and_then(Value::as_f64).unwrap_or(8.0)));
                        ui.checkbox(&mut enabled,"Source zero-phase low-pass");if enabled{ui.add(egui::DragValue::new(&mut hz).range(0.1..=100.0).suffix(" Hz"));}
                        if ui.button("Reload & apply").clicked(){source.settings["low_pass_enabled"]=json!(enabled);source.settings["low_pass_hz"]=json!(hz);reload.push(source.clone());changed=true;}
                        if ui.button("Locate source file…").clicked() && let Some(path)=rfd::FileDialog::new().pick_file(){source.path=path;reload.push(source.clone());changed=true;}
                        if ui.button("Remove source").on_hover_text("Removes its bindings, not files or other sources.").clicked(){removed_sources.push(source.id);changed=true;}
                        // Draft controls must not silently change saved processing.
                        ui.data_mut(|d|d.insert_temp(filter_id,(enabled,hz)));
                    });}
                    if ui.button("Attach another telemetry source…").clicked(){extra=Some(recording.id);}
                    ui.separator();
                    let offset=recording.sources.iter().find(|s|s.id==recording.primary_source).map_or(0.0,|s|s.alignment.offset_seconds);
                    let extent=self.data.get(&recording.primary_source).map(|d|d.raw.channels.values().fold((f64::INFINITY,f64::NEG_INFINITY),|(a,b),c|(a.min(c.series.samples.first().map_or(a,|s|s.time-offset)),b.max(c.series.samples.last().map_or(b,|s|s.time-offset)))));
                    for seg in &mut recording.segments {ui.push_id(seg.id.0,|ui|{
                        ui.label(&seg.name);let mut start=seg.start_recording_time;let mut end=seg.end_recording_time;
                        let mut edit=ui.add(egui::DragValue::new(&mut start).speed(0.05).prefix("Start s ")).changed() | ui.add(egui::DragValue::new(&mut end).speed(0.05).prefix("Finish s ")).changed();
                        if let Some(bounds)=extent {edit|=interval_bar(ui,&mut start,&mut end,bounds);}
                        if edit && start.is_finite() && end.is_finite() && end>start{seg.start_recording_time=start;seg.end_recording_time=end;seg.estimated=true;changed=true;}
                        changed|=ui.checkbox(&mut seg.competitive,"Include as competitive").changed();
                    });}
                    if ui.button("Add manual interval").clicked(){recording.segments.push(RunSegment{id:SegmentId::new(),name:"Manual interval".into(),start_recording_time:0.0,end_recording_time:60.0,kind:SegmentKind::Unknown,estimated:true,competitive:false,unknown:Default::default()});changed=true;}
                });});
            }
            ui.separator();ui.heading("Optional course gates");ui.small("Scrub the reference to the timing line, then capture its position and travel direction. GPS gate durations are estimates, not official timing-system results.");
            let capture=self.reference_gate_capture();
            if let Some((_,name,time,_))=&capture{ui.small(format!("Capture source: {name} at recording {time:.3} s"));}
            ui.horizontal_wrapped(|ui|{
                for(i,label)in ["Capture start / circuit gate","Capture finish gate"].iter().enumerate(){if ui.add_enabled(capture.is_some(),egui::Button::new(*label)).clicked(){let gate=capture.as_ref().unwrap().3;if i==0{if self.workspace.course.gates.is_empty(){self.workspace.course.gates.push(gate);}else{self.workspace.course.gates[0]=gate;}}else if self.workspace.course.gates.len()==1{self.workspace.course.gates.push(gate);}else if self.workspace.course.gates.len()>1{self.workspace.course.gates[1]=gate;}changed=true;}}
                if ui.button("Restore detected intervals").on_hover_text("Replaces gate-created and manually edited intervals with fresh logger-lap or motion detection while preserving the pinned reference when possible.").clicked(){restore_intervals=true;}
                if ui.add_enabled(!self.workspace.course.gates.is_empty(),egui::Button::new("Clear gates & restore intervals")).clicked(){self.workspace.course.gates.clear();restore_intervals=true;changed=true;}
            });
            for (i,gate)in self.workspace.course.gates.iter_mut().enumerate(){ui.horizontal_wrapped(|ui|{ui.label(if i==0{"Start"}else{"Finish"});changed|=ui.add(egui::DragValue::new(&mut gate.latitude).speed(0.00001).max_decimals(7).prefix("Lat ")).changed();changed|=ui.add(egui::DragValue::new(&mut gate.longitude).speed(0.00001).max_decimals(7).prefix("Lon ")).changed();changed|=ui.add(egui::DragValue::new(&mut gate.heading_degrees).speed(0.5).suffix("° heading")).changed();changed|=ui.add(egui::DragValue::new(&mut gate.width_meters).range(1.0..=100.0).suffix(" m width")).changed();});}
            if ui.add_enabled(!self.workspace.course.gates.is_empty(),egui::Button::new("Apply gates to all recordings")).clicked(){self.apply_gates();changed=true;}
            ui.collapsing("Manual course-position anchors",|ui|{
                ui.small("Choose a compared run and give a recording time that corresponds to a known reference-course distance. Anchors affect only that run's GPS matching, not its sensor timestamps.");
                let mut selected=self.workspace.selected.clone().or_else(||self.state.selection.last().cloned());
                egui::ComboBox::from_id_salt("anchor-run").selected_text(selected.as_ref().and_then(|k|segment(&self.workspace,k)).map_or("Run",|(_,s)|s.name.as_str())).show_ui(ui,|ui|{for r in &self.prepared.runs{ui.selectable_value(&mut selected,Some(r.key.clone()),&r.name);}});self.workspace.selected=selected.clone();
                let id=ui.id().with("anchor-pair");let mut pair=ui.data_mut(|d|d.get_temp::<[f64;2]>(id).unwrap_or([0.0,0.0]));
                ui.add(egui::DragValue::new(&mut pair[0]).speed(0.01).prefix("Run recording s "));ui.add(egui::DragValue::new(&mut pair[1]).speed(0.1).prefix("Reference meters "));
                if ui.button("Add anchor").clicked() && let Some(key)=selected{self.workspace.course.manual_anchors.push(ManualAnchor{segment:Some(key),recording_time:pair[0],reference_progress:pair[1],unknown:Default::default()});changed=true;}
                ui.data_mut(|d|d.insert_temp(id,pair));let mut remove=None;for(i,a)in self.workspace.course.manual_anchors.iter().enumerate(){ui.horizontal(|ui|{ui.label(format!("{:.3}s → {:.1}m",a.recording_time,a.reference_progress));if ui.small_button("Remove").clicked(){remove=Some(i);}});}if let Some(i)=remove{self.workspace.course.manual_anchors.remove(i);changed=true;}
            });
        });
        if restore_intervals {
            self.restore_detected_intervals();
            changed = true;
        }
        for id in removed_sources {
            self.data.remove(&id);
            self.loading.remove(&id);
            for r in &mut self.workspace.recordings {
                r.sources.retain(|s| s.id != id);
                if r.primary_source == id {
                    r.primary_source = r.sources.first().map_or_else(SourceId::default, |s| s.id);
                }
                if let Some(p) = &mut r.overlay_snapshot {
                    p.sources.retain(|s| s.id != id);
                    for widget in &mut p.widgets {
                        widget.bindings.retain(|b| b.channel.source_id != id);
                    }
                }
            }
        }
        for source in reload {
            self.enqueue(source);
        }
        if let Some(id) = extra
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Telemetry", &["xrk", "csv", "insv", "lrv"])
                .pick_file()
        {
            let source = SourceConfig {
                id: SourceId::new(),
                name: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                adapter: adapter_for(&path).into(),
                path,
                alignment: Default::default(),
                settings: json!({}),
                unknown: Default::default(),
            };
            if let Some(r) = self.workspace.recordings.iter_mut().find(|r| r.id == id) {
                r.sources.push(source.clone());
            }
            self.enqueue(source);
            changed = true;
        }
        if changed {
            self.changed();
        }
    }
    fn apply_gates(&mut self) {
        let gates = &self.workspace.course.gates;
        for r in &mut self.workspace.recordings {
            let Some(data) = self.data.get(&r.primary_source) else {
                continue;
            };
            let gps = gps_points(&data.raw, r);
            let starts = gate_crossings(&gps, gates[0]);
            let pairs = if gates.len() == 1 {
                starts.windows(2).map(|p| (p[0], p[1])).collect::<Vec<_>>()
            } else {
                let ends = gate_crossings(&gps, gates[1]);
                starts
                    .into_iter()
                    .filter_map(|s| ends.iter().copied().find(|e| *e > s + 1.0).map(|e| (s, e)))
                    .collect()
            };
            if pairs.is_empty() {
                continue;
            }
            let segments = pairs
                .into_iter()
                .enumerate()
                .map(|(i, (start, end))| RunSegment {
                    id: r.segments.get(i).map_or_else(SegmentId::new, |s| s.id),
                    name: format!("GPS run {}", i + 1),
                    start_recording_time: start,
                    end_recording_time: end,
                    kind: if gates.len() == 1 {
                        SegmentKind::Lap
                    } else {
                        SegmentKind::Autocross
                    },
                    estimated: true,
                    competitive: true,
                    unknown: Default::default(),
                })
                .collect();
            r.segments = segments;
        }
        self.default_selection();
        self.state.cursor = 0.0;
        self.state.range = None;
        self.message="Applied gates where valid directed crossings were found; unmatched recordings kept their intervals.".into();
    }
    fn restore_detected_intervals(&mut self) {
        for recording in &mut self.workspace.recordings {
            let Some(data) = self.data.get(&recording.primary_source) else {
                continue;
            };
            let gps = gps_points(&data.raw, recording);
            let detected = auto_segments(&data.raw, recording, &gps);
            replace_segments_preserving_identity(recording, detected);
        }
        self.auto_select_pending = true;
        self.state.cursor = 0.0;
        self.state.range = None;
        self.message = "Restored logger-lap or automatically detected intervals; the analysis range was cleared.".into();
    }
    pub(super) fn reference_gate_capture(&self) -> Option<(RecordingId, String, f64, Gate)> {
        let key = self.workspace.reference.as_ref()?;
        let (recording, interval) = segment(&self.workspace, key)?;
        let data = self.data.get(&recording.primary_source)?;
        let gps = gps_points(&data.raw, recording);
        let time = interval.start_recording_time + self.state.cursor;
        let index = gps.partition_point(|point| point.recording_time < time);
        let point = gps.get(index)?;
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
                    self.layout(0);
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
    fn save(&mut self, as_new: bool) {
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
