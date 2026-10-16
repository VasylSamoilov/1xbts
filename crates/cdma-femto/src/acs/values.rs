use super::*;

pub(super) fn leaf_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

pub(super) fn gps_degrees_minutes_to_decimal(value: &str) -> Option<String> {
    let signed = value.parse::<f64>().ok()?;
    if !signed.is_finite() {
        return None;
    }
    let absolute = signed.abs();
    let degrees = (absolute / 100.0).floor();
    let minutes = absolute - degrees * 100.0;
    if minutes >= 60.0 {
        return None;
    }
    let decimal = degrees + minutes / 60.0;
    let sign = if signed.is_sign_negative() { '-' } else { '+' };
    Some(format!("{sign}{decimal:.6}"))
}

pub(super) fn gps_longitude_to_timezone(value: &str) -> Option<String> {
    let decimal = gps_degrees_minutes_to_decimal(value)?.parse::<f64>().ok()?;
    let offset = (decimal / DEGREES_PER_TIMEZONE_HOUR).round() as i32;
    Some(format!("{offset:+03}:00 {}", timezone_abbreviation(offset)))
}

fn timezone_abbreviation(offset: i32) -> &'static str {
    match offset {
        -10 => "HST",
        -9 => "AKST",
        -8 => "PST",
        -7 => "MST",
        -6 => "CST",
        -5 => "EST",
        _ => "UTC",
    }
}

pub(super) fn gps_distance_meters(
    latitude: f64,
    longitude: f64,
    reference_latitude: f64,
    reference_longitude: f64,
) -> f64 {
    const EARTH_RADIUS_METERS: f64 = 6_371_000.0;
    let latitude = latitude.to_radians();
    let reference_latitude = reference_latitude.to_radians();
    let latitude_delta = reference_latitude - latitude;
    let longitude_delta = (reference_longitude - longitude).to_radians();
    let haversine = (latitude_delta / 2.0).sin().powi(2)
        + latitude.cos() * reference_latitude.cos() * (longitude_delta / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_METERS * haversine.sqrt().asin()
}

pub(super) fn cwmp_values_match(name: &str, expected: &str, actual: &str) -> bool {
    let expected = expected.trim();
    let actual = actual.trim();
    if expected == actual {
        return true;
    }
    if !name.ends_with(ADMIN_STATE_SUFFIX) {
        return false;
    }
    let normalized = |value: &str| match value.to_ascii_lowercase().as_str() {
        "lock" | "locked" => Some(false),
        "unlock" | "unlocked" => Some(true),
        _ => None,
    };
    normalized(expected).is_some_and(|value| normalized(actual) == Some(value))
}

pub(super) fn administrative_states(expected: &str, actual: &str) -> Option<(bool, bool)> {
    let state = |value: &str| match value.trim().to_ascii_lowercase().as_str() {
        "lock" | "locked" => Some(false),
        "unlock" | "unlocked" => Some(true),
        _ => None,
    };
    state(expected).zip(state(actual))
}

pub(super) fn is_sensitive_parameter(name: &str) -> bool {
    let leaf = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    leaf.contains("password") || leaf.ends_with("securitykey") || leaf.ends_with("psk")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gps_longitude_maps_to_rough_local_time_zone() {
        assert_eq!(
            gps_longitude_to_timezone("-11814.400000").as_deref(),
            Some("-08:00 PST")
        );
        assert_eq!(
            gps_longitude_to_timezone("-11142.963867").as_deref(),
            Some("-07:00 MST")
        );
        assert_eq!(
            gps_longitude_to_timezone("+00000.000000").as_deref(),
            Some("+00:00 UTC")
        );
        assert_eq!(gps_longitude_to_timezone("not-a-number"), None);
    }

    #[test]
    fn rejects_invalid_gps_degrees_minutes() {
        assert_eq!(gps_degrees_minutes_to_decimal("+3360.000000"), None);
        assert_eq!(
            gps_degrees_minutes_to_decimal("-07530.000000").as_deref(),
            Some("-75.500000")
        );
    }

    #[test]
    fn compares_airave_administrative_states_semantically() {
        let one_x = "Device.OneX.AdminState";
        let sector = "Device.OneX.OneXSectorAdminState";

        assert!(cwmp_values_match(one_x, "unlock", "Unlocked"));
        assert!(cwmp_values_match(sector, "LOCK", "locked"));
        assert!(!cwmp_values_match(one_x, "unlock", "locked"));
        assert!(!cwmp_values_match("Device.Mode", "unlock", "Unlocked"));
    }
}
