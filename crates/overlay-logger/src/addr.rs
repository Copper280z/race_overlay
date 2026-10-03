use std::{
    fmt, io,
    net::{SocketAddr, ToSocketAddrs},
};

/// Overrides where the logger is reached, as `host[:tcp_port[:udp_port]]`.
///
/// The logger always sits at `11.0.0.1` on its own hotspot; the override is
/// for reaching it through a relay on another machine, or a fake logger.
pub const ADDRESS_ENV: &str = "RACE_OVERLAY_MYCHRON_ADDR";

/// Where a logger answers: TCP for the command protocol, UDP for keepalive
/// and discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAddr {
    pub host: String,
    pub tcp_port: u16,
    pub udp_port: u16,
}

impl Default for DeviceAddr {
    fn default() -> Self {
        Self {
            host: "11.0.0.1".into(),
            tcp_port: 2000,
            udp_port: 36002,
        }
    }
}

impl DeviceAddr {
    /// The default address, unless [`ADDRESS_ENV`] holds a valid override.
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(ADDRESS_ENV) {
            Ok(value) if !value.trim().is_empty() => value.trim().parse(),
            _ => Ok(Self::default()),
        }
    }

    pub fn tcp(&self) -> io::Result<SocketAddr> {
        resolve(&self.host, self.tcp_port)
    }

    pub fn udp(&self) -> io::Result<SocketAddr> {
        resolve(&self.host, self.udp_port)
    }
}

fn resolve(host: &str, port: u16) -> io::Result<SocketAddr> {
    (host, port).to_socket_addrs()?.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("cannot resolve {host}"),
        )
    })
}

impl std::str::FromStr for DeviceAddr {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let defaults = Self::default();
        let mut parts = text.split(':');
        let host = parts.next().unwrap_or_default().trim();
        if host.is_empty() {
            return Err(format!("{text:?}: missing host"));
        }
        let port = |part: Option<&str>, default: u16| match part {
            None | Some("") => Ok(default),
            Some(port) => port
                .parse::<u16>()
                .map_err(|_| format!("{text:?}: {port:?} is not a port")),
        };
        let tcp_port = port(parts.next(), defaults.tcp_port)?;
        let udp_port = port(parts.next(), defaults.udp_port)?;
        if parts.next().is_some() {
            return Err(format!("{text:?}: expected host[:tcp[:udp]]"));
        }
        Ok(Self {
            host: host.to_owned(),
            tcp_port,
            udp_port,
        })
    }
}

impl fmt::Display for DeviceAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.host, self.tcp_port, self.udp_port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_overrides() {
        assert_eq!("pits".parse::<DeviceAddr>().unwrap().tcp_port, 2000);
        let addr = "10.0.0.5:12000:46002".parse::<DeviceAddr>().unwrap();
        assert_eq!(
            (addr.host.as_str(), addr.tcp_port, addr.udp_port),
            ("10.0.0.5", 12000, 46002)
        );
        assert_eq!("h::5".parse::<DeviceAddr>().unwrap().tcp_port, 2000);
        assert!("".parse::<DeviceAddr>().is_err());
        assert!("h:x".parse::<DeviceAddr>().is_err());
        assert!("h:1:2:3".parse::<DeviceAddr>().is_err());
        assert_eq!(addr.to_string().parse::<DeviceAddr>().unwrap(), addr);
    }
}
