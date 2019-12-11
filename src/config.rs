/// Code for reading the event handler config file into memory.
use ini::Ini;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// All of the runtime configuration for the plugin.
#[derive(Debug, Clone)]
pub struct Config {
    pub auth_key: Option<Vec<u8>>,
    pub max_room_size: usize,
    pub max_ccu: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auth_key: None,
            max_room_size: usize::max_value(),
            max_ccu: usize::max_value(),
        }
    }
}

impl Config {
    /// Reads the runtime configuration from an INI config file at the given path, applying defaults for individual
    /// configuration values that aren't present, or returning an error if no readable configuration is present at all.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn Error>> {
        let conf = Ini::load_from_file(path)?;
        let section = conf.section(Some("general")).ok_or("No 'general' section present in the config file.")?;
        let defaults: Config = Default::default();

        let auth_key = match section.get("auth_key") {
            Some(keyfile) => {
                let mut buffer = Vec::<u8>::new();
                let mut file = File::open(keyfile)?;
                file.read_to_end(&mut buffer)?;
                Some(buffer)
            }
            None => None,
        };

        Ok(Self {
            auth_key: auth_key,
            max_room_size: section.get("max_room_size").and_then(|x| x.parse().ok()).unwrap_or(defaults.max_room_size),
            max_ccu: section.get("max_ccu").and_then(|x| x.parse().ok()).unwrap_or(defaults.max_ccu),
        })
    }
}
