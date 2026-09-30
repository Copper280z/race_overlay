//! Bounded INSV trailer reader. Reads original gyro records before telemetry-parser's
//! lens/vehicle orientation transformations; timestamps are converted exactly once.
use super::*;
use prost::Message;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Debug)]
pub struct RawCameraVideo {
    pub camera_model: String,
    pub calibration: DualLensCalibration,
    pub motion: CameraMotion,
    pub motion_error: Option<String>,
    pub first_frame_timestamp: f64,
}

// The wire fields are a subset of telemetry-parser's MIT/Apache Insta360 schema.
// Optical metadata is read separately from normalized telemetry channels.
#[derive(Clone, PartialEq, Message)]
struct Metadata {
    #[prost(string, tag = "1")]
    serial_number: String,
    #[prost(string, tag = "2")]
    camera_type: String,
    #[prost(int32, tag = "15")]
    hdr_state: i32,
    #[prost(string, tag = "22")]
    gamma_mode: String,
    #[prost(uint32, tag = "52")]
    media_rotation: u32,
    #[prost(message, optional, tag = "19")]
    dimension: Option<Dimensions>,
    #[prost(int64, tag = "24")]
    first_frame_timestamp: i64,
    #[prost(message, optional, tag = "27")]
    crop: Option<Crop>,
    #[prost(double, tag = "28")]
    gyro_timestamp: f64,
    #[prost(bool, tag = "29")]
    has_gyro_timestamp: bool,
    #[prost(bool, tag = "42")]
    flowstate_online: bool,
    #[prost(bool, tag = "43")]
    dewarp: bool,
    #[prost(string, tag = "54")]
    offset_v3: String,
    #[prost(uint32, tag = "30")]
    timelapse_interval: u32,
    #[prost(bool, tag = "62")]
    raw_gyro: bool,
    #[prost(message, optional, tag = "65")]
    gyro_config: Option<GyroConfig>,
}
#[derive(Clone, PartialEq, Message)]
struct Dimensions {
    #[prost(uint32, tag = "1")]
    x: u32,
    #[prost(uint32, tag = "2")]
    y: u32,
}
#[derive(Clone, PartialEq, Message)]
struct GyroConfig {
    #[prost(uint32, tag = "1")]
    acc_range: u32,
    #[prost(uint32, tag = "2")]
    gyro_range: u32,
}
#[derive(Clone, PartialEq, Message)]
struct Crop {
    #[prost(uint32, tag = "1")]
    width: u32,
    #[prost(uint32, tag = "2")]
    height: u32,
    #[prost(uint32, tag = "3")]
    cropped_width: u32,
    #[prost(uint32, tag = "4")]
    cropped_height: u32,
    #[prost(uint32, tag = "5")]
    offset_x: u32,
    #[prost(uint32, tag = "6")]
    offset_y: u32,
}
fn io_error(e: impl std::fmt::Display) -> String {
    format!("INSV metadata: {e}")
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("checked record"))
}

fn read_records(path: &Path, include_gyro: bool) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut file = File::open(path).map_err(io_error)?;
    let size = file.metadata().map_err(io_error)?.len();
    if size < 78 {
        return Err("INSV trailer is missing".into());
    }
    file.seek(SeekFrom::End(-72)).map_err(io_error)?;
    let mut header = [0; 72];
    file.read_exact(&mut header).map_err(io_error)?;
    if &header[40..] != b"8db42d694ccc418790edff439fe026bf" {
        return Err("INSV trailer signature is missing".into());
    }
    let total = u32_at(&header, 32) as u64;
    if total < 78 || total > size {
        return Err("Invalid INSV trailer size".into());
    }
    if u32_at(&header, 36) != 3 {
        return Err("Only X4 Air version-3 trailers are supported".into());
    }
    file.seek(SeekFrom::End(-78)).map_err(io_error)?;
    let mut footer = [0; 6];
    file.read_exact(&mut footer).map_err(io_error)?;
    let index_size = u32_at(&footer, 2) as usize;
    if footer[1] != 0
        || index_size > 65536
        || !index_size.is_multiple_of(10)
        || index_size as u64 + 78 > total
    {
        return Err("Invalid INSV trailer index".into());
    }
    file.seek(SeekFrom::End(-78 - index_size as i64))
        .map_err(io_error)?;
    let mut index = vec![0; index_size];
    file.read_exact(&mut index).map_err(io_error)?;
    let mut metadata = None;
    let mut gyro = Vec::new();
    for entry in index.as_chunks::<10>().0 {
        let id = entry[0];
        if id != 1 && !(id == 3 && include_gyro) {
            continue;
        }
        let n = u32_at(entry, 2) as usize;
        let offset = u32_at(entry, 6) as u64;
        let limit = if id == 1 {
            1_048_576
        } else {
            256 * 1024 * 1024
        };
        if n > limit || offset + n as u64 + 6 > total - 72 {
            return Err("INSV record exceeds trailer bounds".into());
        }
        file.seek(SeekFrom::Start(size - total + offset))
            .map_err(io_error)?;
        let mut data = vec![0; n];
        file.read_exact(&mut data).map_err(io_error)?;
        file.read_exact(&mut footer).map_err(io_error)?;
        if footer[0] != entry[1] || footer[1] != id || u32_at(&footer, 2) as usize != n {
            return Err("INSV trailer index disagrees with its record".into());
        }
        if id == 1 {
            if metadata.is_some() {
                return Err("Duplicate INSV metadata record".into());
            }
            metadata = Some(data);
        } else {
            gyro = data;
        }
    }
    Ok((metadata.ok_or("INSV optical metadata is missing")?, gyro))
}

fn calibration(metadata: &Metadata) -> Result<DualLensCalibration, String> {
    if metadata.camera_type != "Insta360 X4 Air" {
        return Err(format!(
            "Raw reprojection currently supports X4 Air; this file is {}",
            metadata.camera_type
        ));
    }
    if metadata.flowstate_online
        || metadata.dewarp
        || metadata.timelapse_interval != 0
        || metadata.hdr_state != 0
        || metadata.media_rotation != 0
        || metadata.gamma_mode != "standard"
    {
        return Err("Only unprocessed standard X4 Air 360 video is supported".into());
    }
    let values = metadata
        .offset_v3
        .split('_')
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Malformed INSV lens calibration")?;
    if values.len() != 40 || values[0] != 2. || !values.iter().all(|v| v.is_finite()) {
        return Err("Expected two complete X4 Air offset_v3 lens calibrations".into());
    }
    let c = metadata
        .crop
        .as_ref()
        .ok_or("INSV sensor crop is missing")?;
    let d = metadata
        .dimension
        .as_ref()
        .ok_or("INSV sensor dimensions are missing")?;
    if c.width == 0
        || c.height == 0
        || c.cropped_width == 0
        || c.cropped_height == 0
        || c.cropped_width > c.width
        || c.cropped_height > c.height
        || d.x == 0
        || d.y == 0
    {
        return Err("Invalid INSV sensor crop dimensions".into());
    }
    let mut lenses = Vec::new();
    for i in 0..2 {
        let v = &values[1 + i * 19..20 + i * 19];
        if v[0] <= 0. || v[1] <= 0. || v[2] <= 0. || v[16] <= 0. || v[17] <= 0. || v[18] != 131. {
            return Err("Unsupported X4 Air lens calibration model".into());
        }
        let w = v[16] * 0.5;
        let h = v[17];
        let crop_w = w * c.cropped_width as f64 / c.width as f64;
        let crop_h = h * c.cropped_height as f64 / c.height as f64;
        let cx = v[3] - i as f64 * w - (w - crop_w) * 0.5 - c.offset_x as f64;
        let cy = v[4] - (h - crop_h) * 0.5 - c.offset_y as f64;
        // X4 Air's calibration frame has a nominal 90-degree sensor rotation.
        // Decode presents both square tracks upright; remove that layout rotation
        // before applying the opposed optical axes.
        let rotation = Quaternion::axis_angle([0., 0., 1.], (v[7] - 90.).to_radians())
            .multiply(Quaternion::axis_angle([0., 1., 0.], v[6].to_radians()))
            .multiply(Quaternion::axis_angle([1., 0., 0.], v[5].to_radians()))
            .multiply(Quaternion::axis_angle(
                [0., 1., 0.],
                i as f64 * std::f64::consts::PI,
            ));
        lenses.push(LensCalibration {
            xi: v[0],
            focal: [v[1] / crop_w, v[2] / crop_h],
            center: [cx / crop_w, cy / crop_h],
            distortion: [v[11], v[12], v[13], v[14], v[15]],
            rig_to_lens: rotation,
            translation: [v[8], v[9], v[10]],
            max_angle: 100_f64.to_radians(),
        });
    }
    Ok(DualLensCalibration {
        lenses: lenses.try_into().expect("two lenses"),
    })
}

pub fn read_insta360_video(path: impl AsRef<Path>) -> Result<RawCameraVideo, String> {
    let (bytes, gyro) = read_records(path.as_ref(), true)?;
    let m = Metadata::decode(bytes.as_slice()).map_err(io_error)?;
    let calibration = calibration(&m)?;
    let gyro_offset = if m.has_gyro_timestamp {
        m.gyro_timestamp * 0.001
    } else {
        0.
    };
    let mut samples = Vec::new();
    let stride = if m.raw_gyro { 20 } else { 56 };
    let mut motion_error =
        (!gyro.len().is_multiple_of(stride)).then(|| "Truncated INSV gyro record".to_owned());
    if !m.raw_gyro && !gyro.is_empty() {
        motion_error = Some("Unsupported X4 Air gyro encoding".into());
    }
    let range = m
        .gyro_config
        .as_ref()
        .map_or(2000., |c| c.gyro_range as f64);
    if !range.is_finite() || !(1.0..=8000.0).contains(&range) {
        motion_error = Some("Invalid INSV gyro range".into());
    }
    let gyro = if motion_error.is_some() {
        &[][..]
    } else {
        &gyro[..]
    };
    for record in gyro.chunks_exact(stride) {
        let stamp = u64::from_le_bytes(record[..8].try_into().expect("timestamp")) as f64;
        let raw: [f64; 3] = std::array::from_fn(|i| {
            if m.raw_gyro {
                let k = 14 + i * 2;
                (u16::from_le_bytes(record[k..k + 2].try_into().expect("gyro")) as f64 - 32768.)
                    * range
                    / 32768.
                    * std::f64::consts::PI
                    / 180.
            } else {
                let k = 32 + i * 8;
                f64::from_le_bytes(record[k..k + 8].try_into().expect("gyro"))
            }
        });
        // X4 Air raw sensor -> decoded optical rig (right, down, front).
        // Validated against inter-frame spherical feature rotations, before any
        // telemetry-parser lens rotation or vehicle calibration.
        let radians_per_second = [raw[2], raw[0], raw[1]];
        let time = (stamp - m.first_frame_timestamp as f64) * if m.raw_gyro { 1e-6 } else { 1e-3 }
            - gyro_offset;
        samples.push(GyroSample {
            time,
            radians_per_second,
        });
    }
    Ok(RawCameraVideo {
        camera_model: m.camera_type,
        calibration,
        motion: CameraMotion { samples },
        motion_error,
        first_frame_timestamp: m.first_frame_timestamp as f64,
    })
}

/// Compare source identities using the embedded camera serial and recording clock.
/// File names alone are not sufficient to associate recordings from different cameras.
pub fn same_camera_recording(a: &Path, b: &Path) -> bool {
    if a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
    {
        return true;
    }
    let metadata = |p: &Path| {
        read_records(p, false)
            .ok()
            .and_then(|(bytes, _)| Metadata::decode(bytes.as_slice()).ok())
    };
    match (metadata(a), metadata(b)) {
        (Some(a), Some(b)) => {
            !a.serial_number.is_empty()
                && a.serial_number == b.serial_number
                && a.camera_type == b.camera_type
                && a.first_frame_timestamp == b.first_frame_timestamp
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn factory_fixture() -> Metadata {
        let mut numbers = vec![2.];
        for lens in 0..2 {
            numbers.extend([
                2.31494,
                7125.,
                7125.,
                3840. + lens as f64 * 7680.,
                3840.,
                0.,
                0.,
                90.,
                0.,
                0.,
                -0.033 * lens as f64,
                0.96,
                -2.04,
                4.27,
                0.,
                0.,
                15360.,
                7680.,
                131.,
            ]);
        }
        numbers.push(197632.);
        Metadata {
            camera_type: "Insta360 X4 Air".into(),
            gamma_mode: "standard".into(),
            serial_number: "synthetic".into(),
            offset_v3: numbers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("_"),
            dimension: Some(Dimensions { x: 3840, y: 3840 }),
            crop: Some(Crop {
                width: 7680,
                height: 7680,
                cropped_width: 7424,
                cropped_height: 7424,
                offset_x: 0,
                offset_y: 0,
            }),
            raw_gyro: true,
            first_frame_timestamp: 1_000_000,
            gyro_timestamp: 1.6,
            has_gyro_timestamp: true,
            ..Default::default()
        }
    }
    fn trailer(metadata: &Metadata, gyro: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut index = Vec::new();
        for (id, format, data) in [
            (1_u8, 1_u8, metadata.encode_to_vec()),
            (3, 0, gyro.to_vec()),
        ] {
            index.extend([id, format]);
            index.extend((data.len() as u32).to_le_bytes());
            index.extend((bytes.len() as u32).to_le_bytes());
            bytes.extend(&data);
            bytes.extend([format, id]);
            bytes.extend((data.len() as u32).to_le_bytes());
        }
        bytes.extend(&index);
        bytes.extend([0, 0]);
        bytes.extend((index.len() as u32).to_le_bytes());
        let total = bytes.len() as u32 + 72;
        bytes.extend([0; 32]);
        bytes.extend(total.to_le_bytes());
        bytes.extend(3_u32.to_le_bytes());
        bytes.extend(b"8db42d694ccc418790edff439fe026bf");
        bytes
    }
    #[test]
    fn factory_layout_has_upright_opposed_lenses_and_real_overlap() {
        let c = calibration(&factory_fixture()).unwrap();
        for (i, ray) in [[0., 0., 1.], [0., 0., -1.]].into_iter().enumerate() {
            let (uv, _) = c.lenses[i].project(ray).unwrap();
            assert!((uv[0] - 0.5).abs() < 1e-9);
            assert!((uv[1] - 0.5).abs() < 1e-9);
        }
        for lens in &c.lenses {
            assert!(lens.project([1., 0., 0.]).is_some());
        }
        assert!(c.lenses[0].project([0., 0.1, 1.]).unwrap().0[1] > 0.5);
        assert!(c.lenses[1].project([0., 0.1, -1.]).unwrap().0[1] > 0.5);
        let mut unsupported = factory_fixture();
        unsupported.hdr_state = 1;
        assert!(calibration(&unsupported).is_err());
    }
    #[test]
    fn trailer_clock_and_sensor_axes_are_applied_once() {
        let m = factory_fixture();
        let mut gyro = 1_101_600_u64.to_le_bytes().to_vec();
        for v in [0_u16, 65535, 30000, 32868, 32968, 33068] {
            gyro.extend(v.to_le_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("camera.insv");
        std::fs::write(&path, trailer(&m, &gyro)).unwrap();
        let raw = read_insta360_video(&path).unwrap();
        let s = raw.motion.samples[0];
        assert!((s.time - 0.1).abs() < 1e-12);
        let scale = 2000. / 32768. * std::f64::consts::PI / 180.;
        assert_eq!(
            s.radians_per_second,
            [300. * scale, 100. * scale, 200. * scale]
        );
        let second = dir.path().join("proxy.lrv");
        std::fs::write(&second, trailer(&m, &gyro)).unwrap();
        assert!(same_camera_recording(&path, &second));
        let mut different = m;
        different.first_frame_timestamp += 1;
        std::fs::write(&second, trailer(&different, &gyro)).unwrap();
        assert!(!same_camera_recording(&path, &second));
    }
    #[test]
    fn rejects_wrong_model_and_truncated_calibration() {
        let mut m = Metadata {
            camera_type: "Insta360 X5".into(),
            ..Default::default()
        };
        assert!(calibration(&m).unwrap_err().contains("X4 Air"));
        m.camera_type = "Insta360 X4 Air".into();
        assert!(calibration(&m).is_err());
    }
    #[test]
    fn short_file_is_a_regular_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(read_insta360_video(f.path()).is_err());
    }
    #[test]
    #[ignore = "reads the optional local X4 Air recording"]
    fn supplied_x4_air_metadata() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VID_20260830_124108_00_017.insv");
        let raw = read_insta360_video(p).unwrap();
        assert_eq!(raw.camera_model, "Insta360 X4 Air");
        assert!(raw.motion.samples.len() > 1000);
        assert!(raw.motion.prepare(0.25).unwrap().covers(0., 180.));
    }
}
