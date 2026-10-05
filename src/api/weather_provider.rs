#[derive(Clone, Debug, PartialEq)]
pub struct DayForecast {
    pub label: String,
    /// Unabbreviated day label for wide panels ("TOMORROW"); empty = use `label`.
    pub label_long: String,
    pub temp: String,
    pub temp_min: String,
    pub temp_max: String,
    pub condition: String,
    /// Unabbreviated condition for wide panels; empty = use `condition`.
    pub condition_long: String,
    pub icon: String,
    /// MQTT Data "NOW" page: `temp_min` holds the live reading (drawn
    /// white), `temp_max` the humidity / wind line (drawn in the condition grey).
    pub is_now: bool,
    /// NOW page: row 2 without the wind direction, used when the full line
    /// doesn't fit; empty when there's no direction to drop.
    pub now_line2_short: String,
}

pub trait WeatherProvider: Send + Sync {
    /// Fetches the weather forecast for the given city, language, and units (metric/imperial).
    /// Returns a vector of DayForecast if successful.
    fn fetch_forecast(
        &self,
        api_key: &str,
        city: &str,
        lang: &str,
        units: &str,
    ) -> Option<Vec<DayForecast>>;
}
