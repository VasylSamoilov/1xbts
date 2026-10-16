use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::config::{
    CwmpInformResponseStyle, CwmpParam, FapAcsConfig, FapDefinition, FapImplementationKind,
    FapProfileConfig, FemtoSiteConfig, ProvisioningStage, SecurityProfile,
};

pub const STANDARD_FEMTO_CONTENT_TYPE: &str = "application/vnd.3gpp2.FemtoInterfaceMsg";
pub const AIRVANA_FEMTO_CONTENT_TYPE: &str = "application/FemtoInterfaceMsg";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmsAckMode {
    ExplicitFemtoMessage,
    CorrelatedEmptyMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FapMobileIdentityCandidate {
    SipFrom(String),
    SipContact(String),
}

impl FapMobileIdentityCandidate {
    pub fn value(&self) -> &str {
        match self {
            Self::SipFrom(value) | Self::SipContact(value) => value,
        }
    }
}

pub trait FapFx2: Send + Sync {
    fn preferred_femto_content_type(&self) -> &'static str;
    fn accepts_femto_content_type(&self, content_type: &str) -> bool;
    fn sms_ack_mode(&self) -> SmsAckMode;
    fn accepts_mobile_identity(&self, candidate: &FapMobileIdentityCandidate) -> bool;
}

pub trait FapProvisioning: Send + Sync {
    fn program_source(&self) -> ProvisioningProgramSource;

    fn sip_password_parameter(&self) -> Option<&'static str> {
        None
    }

    fn radio_administrative_stage(&self, _enabled: bool) -> Option<ProvisioningStage> {
        None
    }

    fn stage_parameters<'a>(
        &self,
        stage: &'a ProvisioningStage,
        pending: &HashSet<String>,
        transition_stage: bool,
    ) -> Vec<&'a CwmpParam>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningProgramSource {
    ProfileOnly,
    DeploymentFallback,
}

pub trait FapSecurity: Send + Sync {
    fn supports(&self, profile: SecurityProfile) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaPacketFormat {
    StandardRtp,
    AirvanaCompact,
}

pub trait FapMedia: Send + Sync {
    fn packet_format(&self) -> MediaPacketFormat;
}

#[derive(Debug, Default, PartialEq)]
pub struct NormalizedFapStatus {
    pub gps_status: Option<String>,
    pub timing_status: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub radio_operational_state: Option<String>,
}

pub trait FapStatusMapping: Send + Sync {
    fn normalize<'a>(
        &self,
        parameters: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> NormalizedFapStatus
    where
        Self: Sized;
}

pub trait FapImplementation: Send + Sync {
    fn kind(&self) -> FapImplementationKind;
    fn fx2(&self) -> &dyn FapFx2;
    fn provisioning(&self) -> &dyn FapProvisioning;
    fn security(&self) -> &dyn FapSecurity;
    fn media(&self) -> &dyn FapMedia;
    fn normalize_status(&self, parameters: &[(&str, &str)]) -> NormalizedFapStatus;
}

#[derive(Debug)]
struct StandardXs0059;

impl FapFx2 for StandardXs0059 {
    fn preferred_femto_content_type(&self) -> &'static str {
        STANDARD_FEMTO_CONTENT_TYPE
    }

    fn accepts_femto_content_type(&self, content_type: &str) -> bool {
        content_type.eq_ignore_ascii_case(STANDARD_FEMTO_CONTENT_TYPE)
    }

    fn sms_ack_mode(&self) -> SmsAckMode {
        SmsAckMode::ExplicitFemtoMessage
    }

    fn accepts_mobile_identity(&self, _candidate: &FapMobileIdentityCandidate) -> bool {
        false
    }
}

impl FapProvisioning for StandardXs0059 {
    fn program_source(&self) -> ProvisioningProgramSource {
        ProvisioningProgramSource::ProfileOnly
    }

    fn stage_parameters<'a>(
        &self,
        stage: &'a ProvisioningStage,
        pending: &HashSet<String>,
        transition_stage: bool,
    ) -> Vec<&'a CwmpParam> {
        let stage_pending = stage
            .params
            .iter()
            .any(|parameter| pending.contains(&parameter.name));
        stage
            .params
            .iter()
            .filter(|parameter| {
                transition_stage
                    || (stage.atomic && stage_pending)
                    || pending.contains(&parameter.name)
            })
            .collect()
    }
}

impl FapSecurity for StandardXs0059 {
    fn supports(&self, profile: SecurityProfile) -> bool {
        profile == SecurityProfile::Standard
    }
}

impl FapMedia for StandardXs0059 {
    fn packet_format(&self) -> MediaPacketFormat {
        MediaPacketFormat::StandardRtp
    }
}

impl FapStatusMapping for StandardXs0059 {
    fn normalize<'a>(
        &self,
        _parameters: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> NormalizedFapStatus {
        NormalizedFapStatus::default()
    }
}

impl FapImplementation for StandardXs0059 {
    fn kind(&self) -> FapImplementationKind {
        FapImplementationKind::StandardXs0059
    }

    fn fx2(&self) -> &dyn FapFx2 {
        self
    }

    fn provisioning(&self) -> &dyn FapProvisioning {
        self
    }

    fn security(&self) -> &dyn FapSecurity {
        self
    }

    fn media(&self) -> &dyn FapMedia {
        self
    }

    fn normalize_status(&self, parameters: &[(&str, &str)]) -> NormalizedFapStatus {
        self.normalize(parameters.iter().copied())
    }
}

#[derive(Debug)]
struct AirvanaAirave2;

impl FapFx2 for AirvanaAirave2 {
    fn preferred_femto_content_type(&self) -> &'static str {
        AIRVANA_FEMTO_CONTENT_TYPE
    }

    fn accepts_femto_content_type(&self, content_type: &str) -> bool {
        content_type.eq_ignore_ascii_case(STANDARD_FEMTO_CONTENT_TYPE)
            || content_type.eq_ignore_ascii_case(AIRVANA_FEMTO_CONTENT_TYPE)
    }

    fn sms_ack_mode(&self) -> SmsAckMode {
        SmsAckMode::CorrelatedEmptyMessage
    }

    fn accepts_mobile_identity(&self, candidate: &FapMobileIdentityCandidate) -> bool {
        matches!(
            candidate,
            FapMobileIdentityCandidate::SipFrom(_) | FapMobileIdentityCandidate::SipContact(_)
        )
    }
}

impl FapProvisioning for AirvanaAirave2 {
    fn program_source(&self) -> ProvisioningProgramSource {
        ProvisioningProgramSource::DeploymentFallback
    }

    fn sip_password_parameter(&self) -> Option<&'static str> {
        Some(
            "InternetGatewayDevice.Services.X_0005B9_OneXMO.\
             X_0005B9_OneXIwfCSRVInfoMO.DefaultSipPassword",
        )
    }

    fn radio_administrative_stage(&self, enabled: bool) -> Option<ProvisioningStage> {
        Some(ProvisioningStage {
            label: if enabled { "unlock" } else { "lock" }.to_string(),
            atomic: true,
            replace_object: None,
            bracket_admin: None,
            params: vec![CwmpParam {
                name: "InternetGatewayDevice.Services.X_0005B9_OneXMO.\
                       X_0005B9_OneXRTTMO.AdminState"
                    .to_string(),
                value: if enabled { "unlock" } else { "lock" }.to_string(),
                value_from: None,
                xsd_type: "xsd:string".to_string(),
                verify: true,
            }],
        })
    }

    fn stage_parameters<'a>(
        &self,
        stage: &'a ProvisioningStage,
        pending: &HashSet<String>,
        transition_stage: bool,
    ) -> Vec<&'a CwmpParam> {
        let stage_pending = stage
            .params
            .iter()
            .any(|parameter| pending.contains(&parameter.name));
        let all_verified_pending = stage
            .params
            .iter()
            .filter(|parameter| parameter.verify)
            .all(|parameter| pending.contains(&parameter.name));
        let mut parameters = stage
            .params
            .iter()
            .filter(|parameter| {
                if parameter.name.ends_with("SecurityKey") {
                    stage.atomic && stage_pending
                } else {
                    transition_stage
                        || (stage.atomic && stage_pending)
                        || pending.contains(&parameter.name)
                        || (!parameter.verify && all_verified_pending)
                }
            })
            .collect::<Vec<_>>();
        parameters.sort_by_key(|parameter| {
            if parameter.name.ends_with("SecurityKey") {
                0
            } else if parameter.name.ends_with("PdsnEndIpAddress") {
                1
            } else if parameter.name.ends_with("SecurityParameterIndex") {
                3
            } else {
                2
            }
        });
        parameters
    }
}

impl FapSecurity for AirvanaAirave2 {
    fn supports(&self, profile: SecurityProfile) -> bool {
        profile == SecurityProfile::Legacy
    }
}

impl FapMedia for AirvanaAirave2 {
    fn packet_format(&self) -> MediaPacketFormat {
        MediaPacketFormat::AirvanaCompact
    }
}

impl FapStatusMapping for AirvanaAirave2 {
    fn normalize<'a>(
        &self,
        parameters: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> NormalizedFapStatus {
        let mut status = NormalizedFapStatus::default();
        for (name, value) in parameters {
            if name.ends_with("GPSGotFix") {
                status.gps_status = Some(match value.to_ascii_lowercase().as_str() {
                    "1" | "true" => "Locked".to_string(),
                    "0" | "false" => "Not locked".to_string(),
                    _ => value.to_string(),
                });
            } else if name.ends_with("TimingGood") {
                status.timing_status = Some(match value.to_ascii_lowercase().as_str() {
                    "1" | "true" => "Synchronized".to_string(),
                    "0" | "false" => "Not synchronized".to_string(),
                    _ => value.to_string(),
                });
            } else if name.ends_with("GPSLatitude") {
                status.gps_latitude = gps_degrees_minutes_to_decimal(value, 90.0);
            } else if name.ends_with("GPSLongitude") {
                status.gps_longitude = gps_degrees_minutes_to_decimal(value, 180.0);
            } else if name.ends_with("OneXSectorOperState") {
                status.radio_operational_state = Some(match value {
                    "0" => "On air".to_string(),
                    "1" => "Off".to_string(),
                    _ => value.to_string(),
                });
            }
        }
        if status.gps_status.as_deref() == Some("Not locked") {
            status.gps_latitude = None;
            status.gps_longitude = None;
        }
        status
    }
}

fn gps_degrees_minutes_to_decimal(value: &str, maximum_degrees: f64) -> Option<f64> {
    let (sign, unsigned) = match value.as_bytes().first() {
        Some(b'-') => (-1.0, &value[1..]),
        Some(b'+') => (1.0, &value[1..]),
        _ => (1.0, value),
    };
    let whole_digits = unsigned.split_once('.')?.0.len();
    if whole_digits < 3 {
        return None;
    }
    let degree_digits = whole_digits - 2;
    let degrees = unsigned.get(..degree_digits)?.parse::<f64>().ok()?;
    let minutes = unsigned.get(degree_digits..)?.parse::<f64>().ok()?;
    if degrees > maximum_degrees || minutes >= 60.0 {
        return None;
    }
    Some(sign * (degrees + minutes / 60.0))
}

impl FapImplementation for AirvanaAirave2 {
    fn kind(&self) -> FapImplementationKind {
        FapImplementationKind::AirvanaAirave2
    }

    fn fx2(&self) -> &dyn FapFx2 {
        self
    }

    fn provisioning(&self) -> &dyn FapProvisioning {
        self
    }

    fn security(&self) -> &dyn FapSecurity {
        self
    }

    fn media(&self) -> &dyn FapMedia {
        self
    }

    fn normalize_status(&self, parameters: &[(&str, &str)]) -> NormalizedFapStatus {
        self.normalize(parameters.iter().copied())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FapRegistryError {
    #[error("SeGW connection references unconfigured FAP {feid}")]
    UnknownFap { feid: String },
    #[error("FAP {feid} references unknown profile {profile}")]
    UnknownProfile { feid: String, profile: String },
    #[error("FAP {feid} references unknown site {site}")]
    UnknownSite { feid: String, site: String },
    #[error("FAP {feid} implementation does not support SeGW profile {profile:?}")]
    UnsupportedSecurityProfile {
        feid: String,
        profile: SecurityProfile,
    },
}

#[derive(Clone)]
pub struct FapRuntimeDefinition {
    pub feid: String,
    pub name: Option<String>,
    pub cwmp_inform_response_style: CwmpInformResponseStyle,
    pub site_name: String,
    pub site: Arc<FemtoSiteConfig>,
    pub implementation: Arc<dyn FapImplementation>,
    pub acs: Option<Arc<FapAcsConfig>>,
}

pub struct FapRegistry {
    by_feid: HashMap<String, FapRuntimeDefinition>,
}

pub type SharedFapRegistry = std::sync::Arc<std::sync::RwLock<std::sync::Arc<FapRegistry>>>;

impl FapRegistry {
    pub fn compatibility(
        feids: impl IntoIterator<Item = String>,
        kind: FapImplementationKind,
    ) -> Self {
        let implementation = implementation(kind);
        let by_feid = feids
            .into_iter()
            .map(|feid| {
                (
                    feid.clone(),
                    FapRuntimeDefinition {
                        feid,
                        name: None,
                        cwmp_inform_response_style: CwmpInformResponseStyle::Standard,
                        site_name: "compatibility".to_string(),
                        site: Arc::new(FemtoSiteConfig {
                            mcc: "310".to_string(),
                            imsi_11_12: "00".to_string(),
                            cell_id: 0x193,
                            sector: 0,
                            ..FemtoSiteConfig::default()
                        }),
                        implementation: implementation.clone(),
                        acs: None,
                    },
                )
            })
            .collect();
        Self { by_feid }
    }

    pub fn from_config(
        profiles: &std::collections::BTreeMap<String, FapProfileConfig>,
        sites: &std::collections::BTreeMap<String, FemtoSiteConfig>,
        faps: &std::collections::BTreeMap<String, FapDefinition>,
    ) -> Result<Self, FapRegistryError> {
        let implementations = profiles
            .iter()
            .map(|(name, profile)| (name.clone(), implementation(profile.implementation)))
            .collect::<HashMap<_, _>>();
        let mut by_feid = HashMap::new();
        for (feid, fap) in faps {
            let selected = implementations.get(&fap.profile).cloned().ok_or_else(|| {
                FapRegistryError::UnknownProfile {
                    feid: feid.clone(),
                    profile: fap.profile.clone(),
                }
            })?;
            let site =
                sites
                    .get(&fap.site)
                    .cloned()
                    .ok_or_else(|| FapRegistryError::UnknownSite {
                        feid: feid.clone(),
                        site: fap.site.clone(),
                    })?;
            by_feid.insert(
                feid.clone(),
                FapRuntimeDefinition {
                    feid: feid.clone(),
                    name: fap.name.clone(),
                    cwmp_inform_response_style: fap.cwmp_inform_response_style,
                    site_name: fap.site.clone(),
                    site: Arc::new(site),
                    implementation: selected,
                    acs: profiles
                        .get(&fap.profile)
                        .and_then(|profile| profile.acs.clone())
                        .map(Arc::new),
                },
            );
        }
        Ok(Self { by_feid })
    }

    pub fn get(&self, feid: &str) -> Option<&FapRuntimeDefinition> {
        self.by_feid.get(feid)
    }

    pub fn require(&self, feid: &str) -> Result<&FapRuntimeDefinition, FapRegistryError> {
        self.get(feid).ok_or_else(|| FapRegistryError::UnknownFap {
            feid: feid.to_string(),
        })
    }

    pub fn len(&self) -> usize {
        self.by_feid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_feid.is_empty()
    }

    pub fn validate_security_profile(
        &self,
        feid: &str,
        profile: SecurityProfile,
    ) -> Result<(), FapRegistryError> {
        let Some(fap) = self.by_feid.get(feid) else {
            return Err(FapRegistryError::UnknownFap {
                feid: feid.to_string(),
            });
        };
        if !fap.implementation.security().supports(profile) {
            return Err(FapRegistryError::UnsupportedSecurityProfile {
                feid: feid.to_string(),
                profile,
            });
        }
        Ok(())
    }
}

pub(crate) fn implementation(kind: FapImplementationKind) -> Arc<dyn FapImplementation> {
    match kind {
        FapImplementationKind::StandardXs0059 => Arc::new(StandardXs0059),
        FapImplementationKind::AirvanaAirave2 => Arc::new(AirvanaAirave2),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn selects_distinct_implementations_for_multiple_faps() {
        let profiles = BTreeMap::from([
            (
                "standard".to_string(),
                FapProfileConfig {
                    implementation: FapImplementationKind::StandardXs0059,
                    connection_request: None,
                    acs: None,
                },
            ),
            (
                "airave".to_string(),
                FapProfileConfig {
                    implementation: FapImplementationKind::AirvanaAirave2,
                    connection_request: None,
                    acs: None,
                },
            ),
        ]);
        let faps = BTreeMap::from([
            (
                "fap-a".to_string(),
                FapDefinition {
                    profile: "standard".to_string(),
                    site: "north".to_string(),
                    name: None,
                    cwmp_inform_response_style: CwmpInformResponseStyle::Standard,
                },
            ),
            (
                "fap-b".to_string(),
                FapDefinition {
                    profile: "airave".to_string(),
                    site: "south".to_string(),
                    name: None,
                    cwmp_inform_response_style: CwmpInformResponseStyle::Standard,
                },
            ),
        ]);

        let sites = BTreeMap::from([
            (
                "north".to_string(),
                FemtoSiteConfig {
                    mcc: "310".to_string(),
                    imsi_11_12: "00".to_string(),
                    cell_id: 403,
                    sector: 0,
                    ..FemtoSiteConfig::default()
                },
            ),
            (
                "south".to_string(),
                FemtoSiteConfig {
                    mcc: "311".to_string(),
                    imsi_11_12: "01".to_string(),
                    cell_id: 404,
                    sector: 1,
                    ..FemtoSiteConfig::default()
                },
            ),
        ]);
        let registry = FapRegistry::from_config(&profiles, &sites, &faps).expect("registry");

        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry.get("fap-a").unwrap().implementation.kind(),
            FapImplementationKind::StandardXs0059
        );
        assert_eq!(
            registry.get("fap-b").unwrap().implementation.kind(),
            FapImplementationKind::AirvanaAirave2
        );
        assert_eq!(registry.get("fap-b").unwrap().site_name, "south");
    }

    #[test]
    fn standard_and_airave_fx2_policies_do_not_leak() {
        let standard = implementation(FapImplementationKind::StandardXs0059);
        let airave = implementation(FapImplementationKind::AirvanaAirave2);

        assert!(
            !standard
                .fx2()
                .accepts_femto_content_type(AIRVANA_FEMTO_CONTENT_TYPE)
        );
        assert!(
            airave
                .fx2()
                .accepts_femto_content_type(AIRVANA_FEMTO_CONTENT_TYPE)
        );
        assert_eq!(
            standard.fx2().sms_ack_mode(),
            SmsAckMode::ExplicitFemtoMessage
        );
        assert_eq!(
            airave.fx2().sms_ack_mode(),
            SmsAckMode::CorrelatedEmptyMessage
        );
        let from = FapMobileIdentityCandidate::SipFrom("5551239999".to_string());
        let contact = FapMobileIdentityCandidate::SipContact("5551239999".to_string());
        assert!(!standard.fx2().accepts_mobile_identity(&from));
        assert!(airave.fx2().accepts_mobile_identity(&from));
        assert!(airave.fx2().accepts_mobile_identity(&contact));
    }

    #[test]
    fn airave_provisioning_order_does_not_affect_standard_profile() {
        let stage = ProvisioningStage {
            label: "packet".to_string(),
            atomic: true,
            replace_object: None,
            bracket_admin: None,
            params: ["PdsnEndIpAddress", "SecurityParameterIndex", "SecurityKey"]
                .into_iter()
                .map(|name| CwmpParam {
                    name: name.to_string(),
                    value: "value".to_string(),
                    value_from: None,
                    xsd_type: "xsd:string".to_string(),
                    verify: true,
                })
                .collect(),
        };
        let pending = stage
            .params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let standard = implementation(FapImplementationKind::StandardXs0059);
        let airave = implementation(FapImplementationKind::AirvanaAirave2);

        assert_eq!(
            standard.provisioning().program_source(),
            ProvisioningProgramSource::ProfileOnly
        );
        assert_eq!(
            airave.provisioning().program_source(),
            ProvisioningProgramSource::DeploymentFallback
        );

        let standard_names = standard
            .provisioning()
            .stage_parameters(&stage, &pending, false)
            .into_iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>();
        let airave_names = airave
            .provisioning()
            .stage_parameters(&stage, &pending, false)
            .into_iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            standard_names,
            vec!["PdsnEndIpAddress", "SecurityParameterIndex", "SecurityKey"]
        );
        assert_eq!(
            airave_names,
            vec!["SecurityKey", "PdsnEndIpAddress", "SecurityParameterIndex"]
        );
    }

    #[test]
    fn status_mapping_is_profile_specific() {
        let parameters = [
            ("Vendor.GPSGotFix", "1"),
            ("Vendor.TimingGood", "true"),
            ("Vendor.GPSLatitude", "+4000.300000"),
            ("Vendor.GPSLongitude", "-07500.600000"),
            ("Vendor.OneXSectorOperState", "0"),
        ];
        let standard = implementation(FapImplementationKind::StandardXs0059);
        let airave = implementation(FapImplementationKind::AirvanaAirave2);

        assert_eq!(
            standard.normalize_status(&parameters),
            NormalizedFapStatus::default()
        );
        assert_eq!(
            airave.normalize_status(&parameters),
            NormalizedFapStatus {
                gps_status: Some("Locked".to_string()),
                timing_status: Some("Synchronized".to_string()),
                gps_latitude: Some(40.005),
                gps_longitude: Some(-75.01),
                radio_operational_state: Some("On air".to_string()),
            }
        );
    }

    #[test]
    fn airave_omits_coordinates_without_a_gps_fix() {
        let parameters = [
            ("Vendor.GPSGotFix", "False"),
            ("Vendor.GPSLatitude", "+0000.000"),
            ("Vendor.GPSLongitude", "+0000.000"),
        ];
        let airave = implementation(FapImplementationKind::AirvanaAirave2);

        assert_eq!(
            airave.normalize_status(&parameters),
            NormalizedFapStatus {
                gps_status: Some("Not locked".to_string()),
                ..NormalizedFapStatus::default()
            }
        );
    }

    #[test]
    fn security_capabilities_are_profile_specific() {
        let standard = implementation(FapImplementationKind::StandardXs0059);
        let airave = implementation(FapImplementationKind::AirvanaAirave2);

        assert!(standard.security().supports(SecurityProfile::Standard));
        assert!(!standard.security().supports(SecurityProfile::Legacy));
        assert!(airave.security().supports(SecurityProfile::Legacy));
        assert!(!airave.security().supports(SecurityProfile::Standard));
        assert_eq!(
            standard.media().packet_format(),
            MediaPacketFormat::StandardRtp
        );
        assert_eq!(
            airave.media().packet_format(),
            MediaPacketFormat::AirvanaCompact
        );
    }

    #[test]
    fn rejects_unknown_profile_reference() {
        let faps = BTreeMap::from([(
            "fap-a".to_string(),
            FapDefinition {
                profile: "missing".to_string(),
                site: "default".to_string(),
                name: None,
                cwmp_inform_response_style: CwmpInformResponseStyle::Standard,
            },
        )]);

        assert_eq!(
            FapRegistry::from_config(&BTreeMap::new(), &BTreeMap::new(), &faps)
                .err()
                .expect("unknown profile"),
            FapRegistryError::UnknownProfile {
                feid: "fap-a".to_string(),
                profile: "missing".to_string(),
            }
        );
    }
}
