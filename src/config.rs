/// Code for reading the event handler config file into memory.
use ini::Ini;
use std::error::Error;
use std::path::Path;

/// All of the runtime configuration for the plugin.
#[derive(Debug, Clone)]
pub struct Config {
    pub max_room_size: usize,
    pub max_ccu: usize
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_room_size: usize::max_value(),
            max_ccu: usize::max_value()
        }
    }
}

impl Config {
    /// Reads the runtime configuration from an INI config file at the given path, applying defaults for individual
    /// configuration values that aren't present, or returning an error if no readable configuration is present at all.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, Box<Error>>
    {
        let conf = Ini::load_from_file(path)?;
        let section = conf.section(Some("general"))
            .ok_or("No 'general' section present in the config file.")?;
        let defaults: Config = Default::default();
        Ok(Self {
            max_room_size: section
                .get("max_room_size")
                .and_then(|x| x.parse().ok())
                .unwrap_or(defaults.max_room_size),
            max_ccu: section
                .get("max_ccu")
                .and_then(|x| x.parse().ok())
                .unwrap_or(defaults.max_ccu),
        })
    }
}
