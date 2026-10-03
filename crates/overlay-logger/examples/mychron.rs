//! Command-line access to a MyChron over Wi-Fi, for protocol work. See
//! [`USAGE`]; the address defaults to `RACE_OVERLAY_MYCHRON_ADDR`, then
//! `11.0.0.1`.

use overlay_logger::{
    ClockWrite, DeviceAddr, MainObject, Session, Timeouts, probe, probe_object, stcp::Request, wifi,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const USAGE: &str = "\
usage: mychron [--addr host[:tcp[:udp]]] <command>

  discover                     UDP keepalive -> descriptor and fingerprint
  probe                        hello, profiles, main object, datalogs, channel blocks
  list                         datalogs as a table
  download <name> [out]        download a recording (default out: ./<name>)
  read <device-path> [out]     download any device path
  stat <device-path>           size of a device path
  info [raw-out]                device info  (WRITES THE LOGGER'S CLOCK)
  set-clock                    both time-sync rounds (WRITES THE LOGGER'S CLOCK)
  live <seconds>               start streaming, poll frames, stop
  object <id> <kind> [path]    one operation on its own connection
  wifi interfaces | permission | scan <if> | current <if> | join <if> <ssid> | restore <if> [ssid]";

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn main() {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut addr = match DeviceAddr::from_env() {
        Ok(addr) => addr,
        Err(error) => exit(&error),
    };
    if args.first().map(String::as_str) == Some("--addr") {
        if args.len() < 2 {
            exit("--addr needs host[:tcp[:udp]]");
        }
        addr = args[1].parse().unwrap_or_else(|error: String| exit(&error));
        args.drain(..2);
    }
    let arg = |i: usize| args.get(i).map(String::as_str);
    let result = match (arg(0), arg(1)) {
        (Some("discover"), _) => discover(&addr),
        (Some("probe"), _) => probe_all(&addr),
        (Some("list"), _) => list(&addr),
        (Some("download"), Some(name)) => {
            download(&addr, &format!("1:/mem/{name}"), arg(2).unwrap_or(name))
        }
        (Some("read"), Some(path)) => {
            let default = path.rsplit('/').next().unwrap_or("device-file");
            download(&addr, path, arg(2).unwrap_or(default))
        }
        (Some("stat"), Some(path)) => stat(&addr, path),
        (Some("info"), raw) => info(&addr, raw),
        (Some("set-clock"), _) => set_clock(&addr),
        (Some("live"), seconds) => live(&addr, seconds.and_then(|s| s.parse().ok()).unwrap_or(10)),
        (Some("object"), Some(id)) => object(&addr, id, arg(2).unwrap_or("2"), arg(3)),
        (Some("wifi"), Some(command)) => wifi_command(command, arg(2), arg(3)),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        exit(&error.to_string());
    }
}

fn exit(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

fn session(addr: &DeviceAddr) -> Result<Session, overlay_logger::LoggerError> {
    Session::open(addr, Timeouts::default())
}

fn discover(addr: &DeviceAddr) -> Fallible {
    match probe(addr, Duration::from_secs(2))? {
        Some(descriptor) => {
            println!("logger at {addr}");
            println!("fingerprint  {}", descriptor.fingerprint());
            println!("identity     {}", hex(descriptor.identity()));
            println!("state bytes  {:?}", descriptor.persisted_state());
        }
        None => println!("no logger answered at {addr}"),
    }
    Ok(())
}

fn probe_all(addr: &DeviceAddr) -> Fallible {
    let mut s = session(addr)?;
    println!("hello ok");
    println!("main object  {:?}", summarize(&s.main_object()?));
    for profile in s.user_profiles()? {
        println!("profile      {:?} ({} B)", profile.name, profile.raw.len());
    }
    println!("datalogs     {}", s.datalogs()?.len());
    let (catalog, channels) = s.channel_catalog()?;
    println!(
        "catalog      hash {:08x}, {} channels",
        catalog.hash,
        channels.len()
    );
    for channel in channels.iter().take(8) {
        println!(
            "  {:>3} {:<8} {}",
            channel.index, channel.short_name, channel.long_name
        );
    }
    let tree = s.channel_tree()?;
    println!(
        "tree         hash {:08x}, {} records",
        tree.hash,
        tree.records.len()
    );
    let (schema, entries) = s.live_schema()?;
    let scaled = entries.iter().filter(|e| e.scale.is_some()).count();
    println!(
        "schema       hash {:08x}, {} records, {scaled} scaled",
        schema.hash,
        entries.len()
    );
    s.close();
    Ok(())
}

fn list(addr: &DeviceAddr) -> Fallible {
    let mut s = session(addr)?;
    for log in s.datalogs()? {
        let best = log
            .best_lap_ms
            .map(|ms| format!("{}.{:03}", ms / 1000, ms % 1000))
            .unwrap_or_default();
        println!(
            "{:<12} {:>9} {:<19} {:>3} laps  best {:>8}  {}",
            log.name,
            log.size,
            log.recorded.map(|t| t.to_string()).unwrap_or_default(),
            log.laps.unwrap_or(0),
            best,
            log.track
        );
    }
    s.close();
    Ok(())
}

fn download(addr: &DeviceAddr, path: &str, out: &str) -> Fallible {
    let mut s = session(addr)?;
    let out = PathBuf::from(out);
    let mut file = BufWriter::new(File::create(&out)?);
    let started = Instant::now();
    let size = s.read_file(
        path,
        &mut file,
        &mut |done, total| eprint!("\r{done}/{total} bytes"),
        &AtomicBool::new(false),
    )?;
    file.flush()?;
    eprintln!();
    println!(
        "{path} → {} ({size} B in {:.1} s)",
        out.display(),
        started.elapsed().as_secs_f64()
    );
    s.close();
    Ok(())
}

fn stat(addr: &DeviceAddr, path: &str) -> Fallible {
    let mut s = session(addr)?;
    match s.stat(path)? {
        Some(size) => println!("{path}: {size} B"),
        None => println!("{path}: absent"),
    }
    s.close();
    Ok(())
}

fn info(addr: &DeviceAddr, raw: Option<&str>) -> Fallible {
    let mut s = session(addr)?;
    let info = s.device_info(ClockWrite::now())?;
    println!("blob      {} B", info.raw.len());
    if let Some(raw) = raw {
        std::fs::write(raw, &info.raw)?;
        println!("saved     {raw}");
    }
    println!(
        "hardware  {:?} {:?}",
        info.hardware.fields, info.hardware.parts
    );
    println!("user      {:?}", info.user);
    for (name, path) in &info.paths {
        println!("path      {name} = {} ({})", path.directory, path.index);
    }
    for record in &info.records {
        println!(
            "record    {:?} {} B checksum {}",
            record.tag,
            record.payload.len(),
            if record.checksum_ok { "ok" } else { "MISMATCH" }
        );
    }
    s.close();
    Ok(())
}

fn set_clock(addr: &DeviceAddr) -> Fallible {
    let mut s = session(addr)?;
    let clock = ClockWrite::now();
    s.device_info(clock)?;
    s.time_negotiate(clock)?;
    println!("clock set to {:?}", clock.local);
    s.close();
    Ok(())
}

fn live(addr: &DeviceAddr, seconds: u64) -> Fallible {
    let mut s = session(addr)?;
    // The schema must be fetched once per boot before frames flow.
    let (schema, _) = s.live_schema()?;
    s.start_live()?;
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut frames = 0;
    while Instant::now() < end {
        match s.main_object()? {
            MainObject::Frame(frame) => {
                frames += 1;
                if frame.schema_hash.is_some_and(|hash| hash != schema.hash) {
                    println!(
                        "frame bound to a different schema {:08x?}",
                        frame.schema_hash
                    );
                }
                let tick = frame.tick_ms.map_or_else(|| "-".into(), |t| t.to_string());
                println!("tick {tick:>10} ms  {} B", frame.raw.len());
            }
            other => println!("{:?}", summarize(&other)),
        }
        s.live_heartbeat()?;
        std::thread::sleep(Duration::from_millis(110));
    }
    s.stop_live()?;
    println!("{frames} frames");
    s.close();
    Ok(())
}

fn object(addr: &DeviceAddr, id: &str, kind: &str, path: Option<&str>) -> Fallible {
    let number = |text: &str| -> Result<u16, Box<dyn std::error::Error>> {
        Ok(match text.strip_prefix("0x") {
            Some(hex) => u16::from_str_radix(hex, 16)?,
            None => text.parse()?,
        })
    };
    let mut request = Request::new(number(id)?, number(kind)?);
    request.path = path.map(str::to_owned);
    match probe_object(addr, Timeouts::default(), &request)? {
        None => println!("no response"),
        Some(result) => {
            println!(
                "status {:?}, length {}",
                result.header.status, result.header.length
            );
            if let Some(data) = result.data {
                println!("{} B: {}", data.len(), hex(&data[..data.len().min(64)]));
            }
        }
    }
    Ok(())
}

fn wifi_command(command: &str, interface: Option<&str>, ssid: Option<&str>) -> Fallible {
    let control = wifi::system().ok_or("no Wi-Fi control on this system")?;
    let need =
        |value: Option<&str>, what: &str| value.map(str::to_owned).ok_or(format!("missing {what}"));
    match command {
        "interfaces" => println!("{}", control.interfaces().join("\n")),
        "permission" => println!("{:?}", control.permission()),
        "scan" => println!(
            "{}",
            control
                .scan_loggers(&need(interface, "interface")?)?
                .join("\n")
        ),
        "current" => println!("{:?}", control.current(&need(interface, "interface")?)?),
        "join" => control.join(&need(interface, "interface")?, &need(ssid, "ssid")?)?,
        "restore" => control.restore(&need(interface, "interface")?, ssid)?,
        other => return Err(format!("unknown wifi command {other}").into()),
    }
    Ok(())
}

fn summarize(object: &MainObject) -> String {
    match object {
        MainObject::Profile { name, .. } => format!("profile {name:?}"),
        MainObject::Frame(frame) => format!("live frame tick {:?}", frame.tick_ms),
        MainObject::Empty => "empty".into(),
        MainObject::Unrecognized(raw) => format!("{} unrecognized bytes", raw.len()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
