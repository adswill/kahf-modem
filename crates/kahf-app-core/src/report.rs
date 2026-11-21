//! Where KAHF's public server is, and the one-degree area names it uses.

pub const SERVER: &str = "https://kahf.dev";

/// The one-degree square a position is in, such as "N51W1". Same rule as the server's `areaKey`.
pub fn area_key(lat: f64, lon: f64) -> String {
    let la = lat.floor() as i32;
    let lo = lon.floor() as i32;
    format!(
        "{}{}{}{}",
        if la >= 0 { "N" } else { "S" },
        la.abs(),
        if lo >= 0 { "E" } else { "W" },
        lo.abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_square() {
        assert_eq!(area_key(51.5, -0.1), "N51W1");
        assert_eq!(area_key(-33.9, 151.2), "S34E151");
    }
}
