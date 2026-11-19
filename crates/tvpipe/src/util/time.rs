//! Time helpers: NTP timestamps and tick conversions.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds between the NTP epoch (1900) and the Unix epoch (1970).
pub const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// A 64-bit NTP timestamp: upper 32 bits seconds since 1900, lower 32 bits
/// fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NtpTime(pub u64);

impl NtpTime {
    pub fn now() -> NtpTime {
        NtpTime::from_system(SystemTime::now())
    }

    pub fn from_system(t: SystemTime) -> NtpTime {
        let d = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let secs = d.as_secs() + NTP_UNIX_OFFSET;
        let frac = ((d.subsec_nanos() as u64) << 32) / 1_000_000_000;
        NtpTime((secs << 32) | frac)
    }

    pub fn seconds(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub fn fraction(self) -> u32 {
        self.0 as u32
    }

    /// The middle 32 bits, as used in RTCP LSR/DLSR fields.
    pub fn compact(self) -> u32 {
        (self.0 >> 16) as u32
    }
}

/// Converts `ticks` at `rate` Hz to another rate, rounding down, without
/// overflowing for any realistic stream length.
pub fn rescale(ticks: u64, from_rate: u32, to_rate: u32) -> u64 {
    if from_rate == 0 {
        return 0;
    }
    ((ticks as u128 * to_rate as u128) / from_rate as u128) as u64
}

pub fn ticks_to_duration(ticks: u64, rate: u32) -> Duration {
    Duration::from_nanos(rescale(ticks, rate, 1_000_000_000))
}

pub fn duration_to_ticks(d: Duration, rate: u32) -> u64 {
    ((d.as_nanos() * rate as u128) / 1_000_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntp_epoch() {
        let t = NtpTime::from_system(UNIX_EPOCH + Duration::from_millis(1500));
        assert_eq!(t.seconds() as u64, NTP_UNIX_OFFSET + 1);
        assert_eq!(t.fraction(), 1 << 31);
    }

    #[test]
    fn rescale_ticks() {
        assert_eq!(rescale(90_000, 90_000, 48_000), 48_000);
        assert_eq!(
            ticks_to_duration(45_000, 90_000),
            Duration::from_millis(500)
        );
        assert_eq!(duration_to_ticks(Duration::from_millis(20), 48_000), 960);
    }
}
