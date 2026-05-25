//! Command-line parsing (port of the `getopt_long` block in `sdl-main.c`),
//! using clap instead of getopt.

use std::path::PathBuf;

use clap::Parser;

const MAX_DIM: i32 = 2048;

/// `risc [OPTIONS...] DISK-IMAGE`
#[derive(Parser, Debug)]
#[command(name = "risc", about = "A Project Oberon RISC5 emulator", version)]
pub struct Cli {
    /// Scale the display in windowed mode
    #[arg(long, value_name = "REAL")]
    zoom: Option<f64>,

    /// Start the emulator in full screen mode
    #[arg(long)]
    fullscreen: bool,

    /// Log LED state on stdout
    #[arg(long)]
    leds: bool,

    /// Set memory size
    #[arg(long, value_name = "MEGS")]
    mem: Option<i32>,

    /// Set framebuffer size
    #[arg(long, value_name = "WIDTHxHEIGHT")]
    size: Option<String>,

    /// Read serial input from FILE
    #[arg(long = "serial-in", value_name = "FILE")]
    serial_in: Option<String>,

    /// Write serial output to FILE
    #[arg(long = "serial-out", value_name = "FILE")]
    serial_out: Option<String>,

    /// Boot from serial line (disk image not required)
    #[arg(long = "boot-from-serial")]
    boot_from_serial: bool,

    #[arg(value_name = "DISK-IMAGE")]
    disk_image: Option<PathBuf>,
}

/// Validated configuration handed to the frontend.
//
// `serial_in`/`serial_out` are consumed once raw serial lands (milestone 8).
#[allow(dead_code)]
pub struct Config {
    pub width: u32,
    pub height: u32,
    pub mem: i32,
    pub configure: bool,
    pub zoom: f64,
    pub fullscreen: bool,
    pub leds: bool,
    pub serial_in: Option<String>,
    pub serial_out: Option<String>,
    pub boot_from_serial: bool,
    pub disk_image: Option<PathBuf>,
}

impl Cli {
    /// Validate options and resolve defaults, mirroring `main`'s argument
    /// handling.
    pub fn into_config(self) -> Result<Config, String> {
        let mut width = crate::risc::FRAMEBUFFER_WIDTH as i32;
        let mut height = crate::risc::FRAMEBUFFER_HEIGHT as i32;
        let size_option = self.size.is_some();
        if let Some(s) = &self.size {
            let (w, h) = parse_size(s)?;
            width = w.clamp(32, MAX_DIM) & !31;
            height = h.clamp(32, MAX_DIM);
        }

        if self.disk_image.is_none() && !self.boot_from_serial {
            return Err("a DISK-IMAGE is required (or pass --boot-from-serial)".into());
        }

        let mem = self.mem.unwrap_or(0);
        Ok(Config {
            width: width as u32,
            height: height as u32,
            mem,
            configure: mem != 0 || size_option,
            zoom: self.zoom.filter(|z| *z > 0.0).unwrap_or(0.0),
            fullscreen: self.fullscreen,
            leds: self.leds,
            serial_in: self.serial_in,
            serial_out: self.serial_out,
            boot_from_serial: self.boot_from_serial,
            disk_image: self.disk_image,
        })
    }
}

fn parse_size(s: &str) -> Result<(i32, i32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("invalid --size {s:?}, expected WIDTHxHEIGHT"))?;
    let w = w.trim().parse::<i32>().map_err(|_| format!("invalid width in --size {s:?}"))?;
    let h = h.trim().parse::<i32>().map_err(|_| format!("invalid height in --size {s:?}"))?;
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parses_and_clamps() {
        assert_eq!(parse_size("800x600").unwrap(), (800, 600));
        assert_eq!(parse_size("1024X768").unwrap(), (1024, 768));
        assert!(parse_size("nonsense").is_err());
    }

    #[test]
    fn requires_disk_image_unless_boot_from_serial() {
        let cli = Cli::parse_from(["risc"]);
        assert!(cli.into_config().is_err());
        let cli = Cli::parse_from(["risc", "--boot-from-serial"]);
        assert!(cli.into_config().is_ok());
        let cli = Cli::parse_from(["risc", "disk.dsk"]);
        let cfg = cli.into_config().unwrap();
        assert_eq!((cfg.width, cfg.height), (1024, 768));
        assert!(!cfg.configure);
    }

    #[test]
    fn width_is_rounded_down_to_multiple_of_32() {
        let cli = Cli::parse_from(["risc", "--size", "1000x700", "disk.dsk"]);
        let cfg = cli.into_config().unwrap();
        assert_eq!(cfg.width, 1000 & !31); // 992
        assert_eq!(cfg.height, 700);
        assert!(cfg.configure);
    }
}
