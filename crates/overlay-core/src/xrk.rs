//! Decoder for AiM XRK recordings.
//!
//! This module keeps file parsing separate from the app adapter and reports
//! packet families it cannot associate with a configured channel.

use std::collections::{BTreeMap, HashMap};

const G0: f64 = 9.80665;

#[derive(Clone, Debug)]
pub(crate) struct XrkChannel {
    pub index: u16,
    pub short_name: String,
    pub long_name: String,
    pub unit: String,
    pub decoder: u8,
    pub interpolate: bool,
    pub samples: Vec<(f64, f64)>,
    pub source_type: u8,
    pub source_channel_id: u16,
    pub sample_period_ms: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct XrkLap {
    pub number: i32,
    pub start: f64,
    pub end: f64,
    pub lap_type: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct XrkDiagnostics {
    pub unconfigured_s: usize,
    pub unconfigured_m: usize,
    pub unconfigured_g: usize,
    pub unrecognized_bytes: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct XrkRecording {
    pub channels: Vec<XrkChannel>,
    pub laps: Vec<XrkLap>,
    pub metadata: BTreeMap<String, String>,
    pub diagnostics: XrkDiagnostics,
}

#[derive(Clone, Debug)]
struct Definition {
    index: u16,
    source_channel_id: u16,
    unit_code: u8,
    source_type: u8,
    decoder: u8,
    short_name: String,
    long_name: String,
    sample_period_ms: u32,
    data_size: usize,
}

#[derive(Clone, Debug)]
struct Header<'a> {
    token: String,
    payload: &'a [u8],
    consumed: usize,
}

pub(crate) fn parse(data: &[u8]) -> Result<XrkRecording, String> {
    if data.len() < 20 {
        return Err("XRK file is too short".into());
    }
    if data.starts_with(&[0x78, 0x01])
        || data.starts_with(&[0x78, 0x5e])
        || data.starts_with(&[0x78, 0x9c])
        || data.starts_with(&[0x78, 0xda])
    {
        return Err("compressed XRZ input is not supported yet; select the original XRK".into());
    }

    let headers = find_all_headers(data);
    let mut definitions = HashMap::<u16, Definition>::new();
    let mut raw_laps = Vec::<(u16, u32, u32)>::new();
    let mut metadata = BTreeMap::new();
    for header in &headers {
        match header.token.as_str() {
            "CHS" => {
                if let Some(definition) = parse_definition(header.payload) {
                    definitions.insert(definition.index, definition);
                }
            }
            "LAP" => {
                if let Some(lap) = parse_lap(header.payload) {
                    raw_laps.push(lap);
                }
            }
            "RCR" => insert_text(&mut metadata, "driver", header.payload),
            "VEH" => insert_text(&mut metadata, "vehicle", header.payload),
            "TMD" => insert_text(&mut metadata, "date", header.payload),
            "TMT" => insert_text(&mut metadata, "time", header.payload),
            "VTY" => insert_text(&mut metadata, "session", header.payload),
            "CMP" => insert_text(&mut metadata, "series", header.payload),
            "NTE" => insert_text(&mut metadata, "comment", header.payload),
            "NDV" => insert_text(&mut metadata, "device", header.payload),
            "TRK" => {
                if let Some(name) = strings(header.payload).into_iter().find(|s| s.len() > 2) {
                    metadata.insert("venue".into(), name);
                }
            }
            _ => {}
        }
    }
    if definitions.is_empty() {
        return Err("no XRK channel definitions were found".into());
    }

    let origin_ms = raw_laps
        .iter()
        .find_map(|(_, duration, end)| (*end >= *duration).then_some((*end - *duration) as i64))
        .unwrap_or(0);
    let mut raw_samples: HashMap<u16, Vec<(i64, f64)>> = HashMap::new();
    let mut diagnostics = XrkDiagnostics::default();
    scan_packets(data, &definitions, &mut raw_samples, &mut diagnostics);

    let mut channels = Vec::new();
    let mut ordered: Vec<_> = definitions.into_values().collect();
    ordered.sort_by_key(|d| d.index);
    for definition in ordered {
        let Some(mut samples) = raw_samples.remove(&definition.index) else {
            continue;
        };
        samples.sort_by_key(|s| s.0);
        samples.dedup_by_key(|s| s.0);
        let samples = samples
            .into_iter()
            .map(|(time, mut value)| {
                if resolved_unit(definition.unit_code) == "V" {
                    value /= 1000.0;
                }
                ((time - origin_ms) as f64 / 1000.0, value)
            })
            .collect();
        channels.push(XrkChannel {
            index: definition.index,
            short_name: definition.short_name,
            long_name: definition.long_name,
            unit: resolved_unit(definition.unit_code).into(),
            decoder: definition.decoder,
            interpolate: decoder_interpolates(definition.decoder),
            samples,
            source_type: definition.source_type,
            source_channel_id: definition.source_channel_id,
            sample_period_ms: definition.sample_period_ms,
        });
    }

    append_gps_channels(data, origin_ms, &mut channels)?;
    let laps = normalize_laps(&raw_laps, origin_ms);
    metadata.insert("format".into(), "AiM XRK".into());
    metadata.insert("configured_channels".into(), channels.len().to_string());
    Ok(XrkRecording {
        channels,
        laps,
        metadata,
        diagnostics,
    })
}

fn find_all_headers(data: &[u8]) -> Vec<Header<'_>> {
    let mut headers = Vec::new();
    let mut pos = 0;
    while pos + 20 <= data.len() {
        if data[pos] == b'<'
            && data[pos + 1] == b'h'
            && let Some(header) = parse_header(data, pos)
        {
            headers.push(header);
        }
        pos += 1;
    }
    headers
}

fn parse_header(data: &[u8], pos: usize) -> Option<Header<'_>> {
    if pos + 20 > data.len() || data.get(pos..pos + 2)? != b"<h" || data[pos + 11] != b'>' {
        return None;
    }
    let length = i32::from_le_bytes(data.get(pos + 6..pos + 10)?.try_into().ok()?);
    if !(0..=64_000_000).contains(&length) {
        return None;
    }
    let length = length as usize;
    let footer = pos.checked_add(12 + length)?;
    let end = footer.checked_add(8)?;
    if end > data.len() || data[footer] != b'<' || data[footer + 7] != b'>' {
        return None;
    }
    if data.get(pos + 2..pos + 6)? != data.get(footer + 1..footer + 5)? {
        return None;
    }
    let token = token_string(data.get(pos + 2..pos + 6)?);
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    Some(Header {
        token,
        payload: &data[pos + 12..footer],
        consumed: end - pos,
    })
}

fn token_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\0', ' '])
        .to_string()
}

fn parse_definition(payload: &[u8]) -> Option<Definition> {
    if payload.len() < 112 {
        return None;
    }
    Some(Definition {
        index: u16_at(payload, 0)?,
        source_channel_id: u16_at(payload, 6)?,
        unit_code: payload[12],
        source_type: payload[16],
        decoder: payload[20],
        short_name: c_string(&payload[24..32]),
        long_name: c_string(&payload[32..56]),
        sample_period_ms: u32_at(payload, 64)? / 1000,
        data_size: payload[72] as usize,
    })
}

fn parse_lap(payload: &[u8]) -> Option<(u16, u32, u32)> {
    if payload.len() < 20 || payload[1] != 0 {
        return None;
    }
    let end_offset = if payload.len() >= 32 { 28 } else { 16 };
    Some((
        u16_at(payload, 2)?,
        u32_at(payload, 4)?,
        u32_at(payload, end_offset)?,
    ))
}

fn normalize_laps(raw: &[(u16, u32, u32)], origin: i64) -> Vec<XrkLap> {
    let mut unique = Vec::<(u16, u32, u32)>::new();
    for &(number, duration, end) in raw {
        if end < duration || unique.last().is_some_and(|x| x.0 == number) {
            continue;
        }
        unique.push((number, duration, end));
    }
    let first_number = unique.first().map_or(0, |x| x.0);
    let count = unique.len();
    unique
        .into_iter()
        .enumerate()
        .map(|(i, (number, duration, end))| {
            let start = end - duration;
            XrkLap {
                number: i32::from(number) - i32::from(first_number),
                start: (i64::from(start) - origin) as f64 / 1000.0,
                end: (i64::from(end) - origin) as f64 / 1000.0,
                lap_type: if i == 0 && i64::from(start) <= origin {
                    "out"
                } else if i + 1 == count {
                    "in"
                } else {
                    "full"
                }
                .into(),
            }
        })
        .collect()
}

fn scan_packets(
    data: &[u8],
    definitions: &HashMap<u16, Definition>,
    output: &mut HashMap<u16, Vec<(i64, f64)>>,
    diagnostics: &mut XrkDiagnostics,
) {
    let mut pos = 0usize;
    while pos + 2 <= data.len() {
        if let Some(header) = parse_header(data, pos) {
            pos += header.consumed;
            continue;
        }
        if data[pos] != b'(' {
            pos += 1;
            continue;
        }
        let consumed = match data[pos + 1] {
            b'S' => parse_s(data, pos, definitions, output, diagnostics),
            b'M' => parse_m(data, pos, definitions, output, diagnostics),
            b'c' => parse_c(data, pos, definitions, output),
            b'G' => {
                diagnostics.unconfigured_g += 1;
                packet_end_by_sizes(data, pos, &[1, 2, 4, 8, 12, 16, 24, 32])
            }
            _ => None,
        };
        if let Some(consumed) = consumed {
            pos += consumed;
        } else {
            diagnostics.unrecognized_bytes += 1;
            pos += 1;
        }
    }
}

fn parse_s(
    data: &[u8],
    pos: usize,
    definitions: &HashMap<u16, Definition>,
    output: &mut HashMap<u16, Vec<(i64, f64)>>,
    diagnostics: &mut XrkDiagnostics,
) -> Option<usize> {
    if pos + 9 > data.len() {
        return None;
    }
    let time = i32_at(data, pos + 2)? as i64;
    let index = u16_at(data, pos + 6)?;
    if let Some(definition) = definitions.get(&index) {
        let total = 9 + definition.data_size;
        if pos + total <= data.len() && data[pos + total - 1] == b')' {
            if let Some(value) = decode_value(definition, &data[pos + 8..pos + total - 1]) {
                output.entry(index).or_default().push((time, value));
            }
            return Some(total);
        }
    }
    diagnostics.unconfigured_s += 1;
    packet_end_by_sizes(data, pos, &[1, 2, 4, 8])
}

fn parse_m(
    data: &[u8],
    pos: usize,
    definitions: &HashMap<u16, Definition>,
    output: &mut HashMap<u16, Vec<(i64, f64)>>,
    diagnostics: &mut XrkDiagnostics,
) -> Option<usize> {
    if pos + 11 > data.len() {
        return None;
    }
    let time = i32_at(data, pos + 2)? as i64;
    let index = u16_at(data, pos + 6)?;
    let count = u16_at(data, pos + 8)? as usize;
    if count > 4096 {
        return None;
    }
    if let Some(definition) = definitions.get(&index) {
        let total = 11usize.checked_add(count.checked_mul(definition.data_size)?)?;
        if definition.sample_period_ms > 0
            && pos + total <= data.len()
            && data[pos + total - 1] == b')'
        {
            let samples = output.entry(index).or_default();
            for sample in 0..count {
                let start = pos + 10 + sample * definition.data_size;
                if let Some(value) =
                    decode_value(definition, &data[start..start + definition.data_size])
                {
                    samples.push((
                        time + sample as i64 * i64::from(definition.sample_period_ms),
                        value,
                    ));
                }
            }
            return Some(total);
        }
    }
    diagnostics.unconfigured_m += 1;
    for size in [1usize, 2, 4, 8] {
        let total = 11usize.checked_add(count.checked_mul(size)?)?;
        if pos + total <= data.len() && data[pos + total - 1] == b')' {
            return Some(total);
        }
    }
    None
}

fn parse_c(
    data: &[u8],
    pos: usize,
    definitions: &HashMap<u16, Definition>,
    output: &mut HashMap<u16, Vec<(i64, f64)>>,
) -> Option<usize> {
    if pos + 12 > data.len() || data[pos + 2] != 0 || data[pos + 5] != 0x84 {
        return match (data.get(pos + 2), data.get(pos + 6)) {
            (Some(0), Some(8)) if pos + 16 <= data.len() && data[pos + 15] == b')' => Some(16),
            (Some(1), Some(2)) if pos + 10 <= data.len() && data[pos + 9] == b')' => Some(10),
            _ => None,
        };
    }
    let field = u16_at(data, pos + 3)?;
    if data[pos + 6] != 6 || field & 7 != 4 {
        return None;
    }
    let index = field >> 3;
    let definition = definitions.get(&index)?;
    let total = definition.data_size + 12;
    if pos + total > data.len() || data[pos + total - 1] != b')' {
        return None;
    }
    let time = i32_at(data, pos + 7)? as i64;
    let value = decode_value(definition, &data[pos + 11..pos + total - 1])?;
    output.entry(index).or_default().push((time, value));
    Some(total)
}

fn packet_end_by_sizes(data: &[u8], pos: usize, sizes: &[usize]) -> Option<usize> {
    for &size in sizes {
        let total = 9 + size;
        if pos + total <= data.len() && data[pos + total - 1] == b')' {
            return Some(total);
        }
    }
    None
}

fn decode_value(definition: &Definition, data: &[u8]) -> Option<f64> {
    if definition.long_name == "Calculated_Gear" && data.len() >= 8 {
        let bits = u64::from_le_bytes(data[..8].try_into().ok()?);
        return Some(if bits & 0x80000 != 0 {
            0.0
        } else {
            ((bits >> 16) & 7) as f64
        });
    }
    Some(match definition.decoder {
        0 | 3 | 8 | 12 | 22 | 24 | 26 | 27 | 31 | 32 | 33 | 37 | 38 | 39 => {
            i32::from_le_bytes(data.get(..4)?.try_into().ok()?) as f64
        }
        1 => u16::from_le_bytes(data.get(..2)?.try_into().ok()?) as f64,
        4 | 11 => i16::from_le_bytes(data.get(..2)?.try_into().ok()?) as f64,
        6 => f32::from_le_bytes(data.get(..4)?.try_into().ok()?) as f64,
        13 => *data.first()? as f64,
        15 => match u16::from_le_bytes(data.get(..2)?.try_into().ok()?) {
            0x4e => 0.0,
            v @ 0x31..=0x36 => f64::from(v - 0x30),
            v => f64::from(v),
        },
        20 => half_to_f32(u16::from_le_bytes(data.get(..2)?.try_into().ok()?)) as f64,
        _ => return None,
    })
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits >> 15) << 31;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let fraction = u32::from(bits & 0x03ff);
    let converted = if exponent == 0 {
        if fraction == 0 {
            sign
        } else {
            let leading = fraction.leading_zeros() - 22;
            let normalized = (fraction << leading) & 0x03ff;
            let exp = 127u32 - 14 - leading;
            sign | (exp << 23) | (normalized << 13)
        }
    } else if exponent == 31 {
        sign | 0x7f80_0000 | (fraction << 13)
    } else {
        sign | ((exponent + 112) << 23) | (fraction << 13)
    };
    f32::from_bits(converted)
}

fn decoder_interpolates(decoder: u8) -> bool {
    matches!(decoder, 1 | 6 | 20)
}

fn resolved_unit(code: u8) -> &'static str {
    let base = match code & 0x7f {
        1 | 33 => "%",
        3 => "g",
        4 => "deg",
        5 => "deg/s",
        9 => "Hz",
        12 => "mm",
        14 => "bar",
        15 => "rpm",
        16 | 20 => "km/h",
        17 => "C",
        18 => "ms",
        19 => "Nm",
        21 => "mV",
        22 => "l",
        24 => "l/s",
        27 => "A",
        30 => "lambda",
        31 => "gear",
        43 => "kg",
        _ => "",
    };
    if code & 0x80 != 0 && base == "mV" {
        "V"
    } else {
        base
    }
}

fn append_gps_channels(
    data: &[u8],
    origin: i64,
    channels: &mut Vec<XrkChannel>,
) -> Result<(), String> {
    let mut records = Vec::new();
    for header in find_all_headers(data) {
        if (header.token == "GPS" || header.token == "GPS1") && header.payload.len() == 56 {
            records.push(header.payload);
        }
    }
    if records.is_empty() {
        return Ok(());
    }
    let mut times = Vec::with_capacity(records.len());
    let mut speed = Vec::with_capacity(records.len());
    let mut latitude = Vec::with_capacity(records.len());
    let mut longitude = Vec::with_capacity(records.len());
    let mut altitude = Vec::with_capacity(records.len());
    let mut satellites = Vec::with_capacity(records.len());
    let mut fix = Vec::with_capacity(records.len());
    let mut pdop = Vec::with_capacity(records.len());
    let mut pos_accuracy = Vec::with_capacity(records.len());
    let mut vel_accuracy = Vec::with_capacity(records.len());
    let mut heading = Vec::with_capacity(records.len());
    for record in records {
        let time = i32_at(record, 0).ok_or("truncated GPS timestamp")? as i64;
        let x = i32_at(record, 16).ok_or("truncated GPS ECEF X")? as f64 / 100.0;
        let y = i32_at(record, 20).ok_or("truncated GPS ECEF Y")? as f64 / 100.0;
        let z = i32_at(record, 24).ok_or("truncated GPS ECEF Z")? as f64 / 100.0;
        let vx = i32_at(record, 32).ok_or("truncated GPS velocity X")? as f64 / 100.0;
        let vy = i32_at(record, 36).ok_or("truncated GPS velocity Y")? as f64 / 100.0;
        let vz = i32_at(record, 40).ok_or("truncated GPS velocity Z")? as f64 / 100.0;
        let (lat, lon, alt) = ecef_to_lla(x, y, z);
        let lat_r = lat.to_radians();
        let lon_r = lon.to_radians();
        let east = -lon_r.sin() * vx + lon_r.cos() * vy;
        let north =
            -lat_r.sin() * lon_r.cos() * vx - lat_r.sin() * lon_r.sin() * vy + lat_r.cos() * vz;
        times.push(time);
        speed.push((vx * vx + vy * vy + vz * vz).sqrt());
        latitude.push(lat);
        longitude.push(lon);
        altitude.push(alt);
        satellites.push(record[51] as f64);
        fix.push(record[14] as f64);
        pdop.push(u16_at(record, 48).unwrap_or(0) as f64 / 100.0);
        pos_accuracy.push(u32_at(record, 28).unwrap_or(0) as f64 / 100.0);
        vel_accuracy.push(u32_at(record, 44).unwrap_or(0) as f64 / 100.0);
        heading.push(east.atan2(north));
    }
    let mut inline = vec![0.0; times.len()];
    let mut lateral = vec![0.0; times.len()];
    let mut yaw_rate = vec![0.0; times.len()];
    for i in 1..times.len() {
        let dt = (times[i] - times[i - 1]) as f64 / 1000.0;
        if dt <= 0.0 || dt > 1.0 {
            continue;
        }
        let mut dh = heading[i] - heading[i - 1];
        while dh > std::f64::consts::PI {
            dh -= std::f64::consts::TAU;
        }
        while dh < -std::f64::consts::PI {
            dh += std::f64::consts::TAU;
        }
        inline[i] = (speed[i] - speed[i - 1]) / dt / G0;
        // Course-over-ground is undefined at rest; suppress the resulting
        // 180-degree jumps rather than presenting them as real yaw motion.
        if speed[i].min(speed[i - 1]) >= 1.0 {
            lateral[i] = speed[i] * dh / dt / G0;
            yaw_rate[i] = dh.to_degrees() / dt;
        }
    }
    if times.len() > 1 {
        inline[0] = inline[1];
        lateral[0] = lateral[1];
        yaw_rate[0] = yaw_rate[1];
    }
    let values = [
        ("gps_speed", "m/s", speed, true),
        ("gps_latitude", "deg", latitude, true),
        ("gps_longitude", "deg", longitude, true),
        ("gps_altitude", "m", altitude, true),
        ("gps_satellites", "", satellites, false),
        ("gps_fix", "", fix, false),
        ("gps_pdop", "", pdop, false),
        ("gps_position_accuracy", "m", pos_accuracy, true),
        ("gps_velocity_accuracy", "m/s", vel_accuracy, true),
        ("gps_inline_acceleration", "g", inline, true),
        ("gps_lateral_acceleration", "g", lateral, true),
        ("gps_yaw_rate", "deg/s", yaw_rate, true),
    ];
    for (offset, (name, unit, values, interpolate)) in values.into_iter().enumerate() {
        channels.push(XrkChannel {
            index: 0xf000 + offset as u16,
            short_name: name.into(),
            long_name: name.into(),
            unit: unit.into(),
            decoder: 6,
            interpolate,
            samples: times
                .iter()
                .zip(values)
                .map(|(&time, value)| ((time - origin) as f64 / 1000.0, value))
                .collect(),
            source_type: 5,
            source_channel_id: offset as u16,
            sample_period_ms: 40,
        });
    }
    Ok(())
}

fn ecef_to_lla(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    const A: f64 = 6_378_137.0;
    const E2: f64 = 6.694_379_990_14e-3;
    let lon = y.atan2(x);
    let p = x.hypot(y);
    let mut lat = z.atan2(p * (1.0 - E2));
    let mut alt = 0.0;
    for _ in 0..8 {
        let sin = lat.sin();
        let n = A / (1.0 - E2 * sin * sin).sqrt();
        alt = p / lat.cos() - n;
        lat = z.atan2(p * (1.0 - E2 * n / (n + alt)));
    }
    (lat.to_degrees(), lon.to_degrees(), alt)
}

fn insert_text(map: &mut BTreeMap<String, String>, key: &str, payload: &[u8]) {
    let value = c_string(payload);
    if !value.is_empty() {
        map.insert(key.into(), value);
    }
}

fn strings(data: &[u8]) -> Vec<String> {
    data.split(|b| *b == 0)
        .filter(|s| s.len() >= 2 && s.iter().all(u8::is_ascii))
        .map(|s| String::from_utf8_lossy(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn c_string(data: &[u8]) -> String {
    let end = data.iter().position(|b| *b == 0).unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).trim().to_string()
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}
fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
fn i32_at(data: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_precision() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xbc00), -1.0);
        assert_eq!(half_to_f32(0), 0.0);
    }
}
