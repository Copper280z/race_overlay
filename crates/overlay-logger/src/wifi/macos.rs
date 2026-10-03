//! CoreWLAN. macOS hides network names (they read as `nil`) unless the app
//! holds Location permission, so scanning and reading the current network
//! need it; joining a scanned open network does not need a password.
//!
//! macOS keeps a single association per adapter: joining the logger takes the
//! Mac off its previous network until [`WifiControl::restore`].

use super::{Permission, WifiControl, WifiError, is_logger_ssid};
use objc2::rc::Retained;
use objc2_core_location::{CLAuthorizationStatus, CLLocationManager};
use objc2_core_wlan::{CWInterface, CWNetwork, CWWiFiClient};
use objc2_foundation::NSString;
use std::cell::RefCell;

pub struct CoreWlan;

thread_local! {
    /// A manager must outlive its authorization prompt.
    static LOCATION: RefCell<Option<Retained<CLLocationManager>>> = const { RefCell::new(None) };
}

fn interface(name: &str) -> Result<Retained<CWInterface>, WifiError> {
    let client = unsafe { CWWiFiClient::sharedWiFiClient() };
    unsafe { client.interfaceWithName(Some(&NSString::from_str(name))) }
        .ok_or_else(|| WifiError::NoInterface(name.to_owned()))
}

fn scan(
    interface: &CWInterface,
    name: Option<&str>,
) -> Result<Vec<Retained<CWNetwork>>, WifiError> {
    let name = name.map(NSString::from_str);
    let networks = unsafe { interface.scanForNetworksWithName_error(name.as_deref()) }
        .map_err(|error| WifiError::Failed(error.localizedDescription().to_string()))?;
    Ok(networks.iter().collect())
}

fn ssid(network: &CWNetwork) -> Option<String> {
    unsafe { network.ssid() }.map(|ssid| ssid.to_string())
}

impl CoreWlan {
    fn require_permission(&self) -> Result<(), WifiError> {
        if self.permission().allows_scanning() {
            Ok(())
        } else {
            Err(WifiError::PermissionDenied)
        }
    }

    fn associate(&self, interface_name: &str, target: &str) -> Result<(), WifiError> {
        self.require_permission()?;
        let interface = interface(interface_name)?;
        let network = scan(&interface, Some(target))?
            .into_iter()
            .find(|network| ssid(network).as_deref() == Some(target))
            .ok_or_else(|| WifiError::NotFound(target.to_owned()))?;
        unsafe { interface.associateToNetwork_password_error(&network, None) }
            .map_err(|error| WifiError::Failed(error.localizedDescription().to_string()))
    }
}

impl WifiControl for CoreWlan {
    fn interfaces(&self) -> Vec<String> {
        let client = unsafe { CWWiFiClient::sharedWiFiClient() };
        unsafe { client.interfaceNames() }
            .map(|names| names.iter().map(|name| name.to_string()).collect())
            .unwrap_or_default()
    }

    fn permission(&self) -> Permission {
        #[allow(deprecated)]
        let status = unsafe { CLLocationManager::authorizationStatus_class() };
        match status {
            CLAuthorizationStatus::AuthorizedAlways
            | CLAuthorizationStatus::AuthorizedWhenInUse => Permission::Granted,
            CLAuthorizationStatus::NotDetermined => Permission::NotDetermined,
            _ => Permission::Denied,
        }
    }

    fn request_permission(&self) {
        LOCATION.with(|slot| {
            let mut slot = slot.borrow_mut();
            let manager = slot.get_or_insert_with(|| unsafe { CLLocationManager::new() });
            unsafe { manager.requestWhenInUseAuthorization() };
        });
    }

    fn scan_loggers(&self, interface_name: &str) -> Result<Vec<String>, WifiError> {
        self.require_permission()?;
        let interface = interface(interface_name)?;
        let mut ssids = scan(&interface, None)?
            .iter()
            .filter_map(|network| ssid(network))
            .filter(|name| is_logger_ssid(name))
            .collect::<Vec<_>>();
        ssids.sort();
        ssids.dedup();
        Ok(ssids)
    }

    fn current(&self, interface_name: &str) -> Result<Option<String>, WifiError> {
        self.require_permission()?;
        Ok(unsafe { interface(interface_name)?.ssid() }.map(|ssid| ssid.to_string()))
    }

    fn join(&self, interface_name: &str, ssid: &str) -> Result<(), WifiError> {
        self.associate(interface_name, ssid)
    }

    fn restore(&self, interface_name: &str, previous: Option<&str>) -> Result<(), WifiError> {
        // A saved network's password is in the system keychain, out of reach;
        // when re-associating without it fails, leaving the logger lets
        // macOS auto-join its preferred network.
        if let Some(previous) = previous
            && self.associate(interface_name, previous).is_ok()
        {
            return Ok(());
        }
        unsafe { interface(interface_name)?.disassociate() };
        Ok(())
    }
}
