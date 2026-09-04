use overlay_core::NormalizedRect;
use overlay_render::{DatasetContext, RenderOptions, RenderSize, Renderer, Rgba, Widget};

fn widget(
    kind: &str,
    slot: &str,
    rect: NormalizedRect,
    label: &str,
    unit: &str,
    range: (f64, f64),
) -> Widget {
    let mut widget = Widget::new(kind, rect);
    widget.value_slot = slot.into();
    widget.label = label.into();
    widget.unit = unit.into();
    widget.min = Some(range.0);
    widget.max = Some(range.1);
    widget.background = Rgba(8, 13, 21, 225);
    widget.accent = Rgba(0, 218, 255, 255);
    widget
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "widget-gallery.png".into());
    let mut data = DatasetContext::default();
    for (slot, value) in [
        ("rpm", 12_750.0),
        ("water", 82.4),
        ("egt", 642.0),
        ("lap", 47.283),
        ("delta", -0.317),
        ("steering", -31.5),
        ("speed", 104.0),
        ("gear", 5.0),
    ] {
        data.insert(slot, [(0.0, value)]);
    }
    data.insert("gx", [(0.0, 1.18)]);
    data.insert("gy", [(0.0, -0.42)]);
    let course = [
        (42.0000, -71.0000),
        (42.0004, -70.9995),
        (42.0006, -70.9988),
        (42.0003, -70.9982),
        (41.9998, -70.9981),
        (41.9995, -70.9987),
        (41.9996, -70.9995),
        (42.0000, -71.0000),
    ];
    data.insert(
        "latitude",
        course
            .iter()
            .enumerate()
            .map(|(index, point)| (index as f64, point.0)),
    );
    data.insert(
        "longitude",
        course
            .iter()
            .enumerate()
            .map(|(index, point)| (index as f64, point.1)),
    );
    let mut g = widget(
        "xy_dot",
        "unused",
        NormalizedRect::new(0.03, 0.58, 0.20, 0.36),
        "G FORCE",
        "g",
        (-2.0, 2.0),
    );
    g.x_slot = "gx".into();
    g.y_slot = "gy".into();
    let mut track = widget(
        "track_map",
        "unused",
        NormalizedRect::new(0.34, 0.31, 0.32, 0.24),
        "TRACK MAP",
        "",
        (0.0, 1.0),
    );
    track.latitude_slot = "latitude".into();
    track.longitude_slot = "longitude".into();
    let widgets = vec![
        widget(
            "shift_lights",
            "rpm",
            NormalizedRect::new(0.31, 0.03, 0.38, 0.10),
            "SHIFT",
            "rpm",
            (7_000.0, 14_000.0),
        ),
        widget(
            "lap_timer",
            "lap",
            NormalizedRect::new(0.03, 0.04, 0.20, 0.14),
            "LAP",
            "",
            (0.0, 120.0),
        ),
        widget(
            "delta",
            "delta",
            NormalizedRect::new(0.77, 0.04, 0.20, 0.14),
            "DELTA",
            "s",
            (-5.0, 5.0),
        ),
        widget(
            "tachometer",
            "rpm",
            NormalizedRect::new(0.35, 0.58, 0.30, 0.36),
            "RPM",
            "",
            (0.0, 16_000.0),
        ),
        widget(
            "temperature",
            "water",
            NormalizedRect::new(0.25, 0.74, 0.09, 0.20),
            "WATER",
            "°C",
            (40.0, 120.0),
        ),
        widget(
            "temperature",
            "egt",
            NormalizedRect::new(0.66, 0.74, 0.09, 0.20),
            "EGT",
            "°C",
            (0.0, 800.0),
        ),
        widget(
            "radial",
            "speed",
            NormalizedRect::new(0.77, 0.58, 0.20, 0.36),
            "SPEED",
            "km/h",
            (0.0, 180.0),
        ),
        widget(
            "gear",
            "gear",
            NormalizedRect::new(0.71, 0.33, 0.08, 0.13),
            "GEAR",
            "",
            (0.0, 6.0),
        ),
        track,
        widget(
            "center_bar",
            "steering",
            NormalizedRect::new(0.31, 0.17, 0.38, 0.12),
            "STEERING",
            "°",
            (-180.0, 180.0),
        ),
        g,
    ];
    let rendered = Renderer::default().render(
        &widgets,
        RenderSize::new(1920, 1080),
        &data,
        0.0,
        RenderOptions {
            crop: false,
            full_size: false,
        },
    );
    image::save_buffer_with_format(
        &output,
        &rendered.image.pixels,
        rendered.image.width,
        rendered.image.height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .expect("write widget gallery");
    println!("{output}");
}
