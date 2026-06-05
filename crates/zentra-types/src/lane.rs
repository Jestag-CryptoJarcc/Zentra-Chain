//! Mining lane identifiers for the 5-lane Multi-Algorithm PoW system.

use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Serialize, Deserialize};
use std::fmt;

/// Identifies one of the 5 parallel mining lanes, each targeting different hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
         BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
#[repr(u8)]
#[borsh(use_discriminant = true)]
pub enum LaneId {
    /// Lane 0: CPU mining — RandomX algorithm
    Cpu = 0,
    /// Lane 1: GPU mining — KawPow algorithm
    Gpu = 1,
    /// Lane 2: Bitcoin ASIC mining — SHA-256 algorithm
    BtcAsic = 2,
    /// Lane 3: Litecoin ASIC mining — Scrypt algorithm
    LtcAsic = 3,
    /// Lane 4: FPGA mining — Yescrypt algorithm
    Fpga = 4,
}

impl LaneId {
    /// All lane IDs in order.
    pub const ALL: [LaneId; 5] = [
        LaneId::Cpu,
        LaneId::Gpu,
        LaneId::BtcAsic,
        LaneId::LtcAsic,
        LaneId::Fpga,
    ];

    /// Convert from a u8 value.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(LaneId::Cpu),
            1 => Some(LaneId::Gpu),
            2 => Some(LaneId::BtcAsic),
            3 => Some(LaneId::LtcAsic),
            4 => Some(LaneId::Fpga),
            _ => None,
        }
    }

    /// Get the u8 representation.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Get the algorithm name for this lane.
    pub fn algorithm_name(&self) -> &'static str {
        match self {
            LaneId::Cpu => "RandomX",
            LaneId::Gpu => "KawPow",
            LaneId::BtcAsic => "SHA-256",
            LaneId::LtcAsic => "Scrypt",
            LaneId::Fpga => "Yescrypt",
        }
    }

    /// Get the hardware target name.
    pub fn hardware_name(&self) -> &'static str {
        match self {
            LaneId::Cpu => "CPU",
            LaneId::Gpu => "GPU",
            LaneId::BtcAsic => "BTC ASIC",
            LaneId::LtcAsic => "LTC ASIC",
            LaneId::Fpga => "FPGA",
        }
    }
}

impl fmt::Display for LaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lane {} ({}/{})", self.as_u8(), self.hardware_name(), self.algorithm_name())
    }
}

impl TryFrom<u8> for LaneId {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, ()> {
        LaneId::from_u8(value).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lane_ids() {
        assert_eq!(LaneId::Cpu.as_u8(), 0);
        assert_eq!(LaneId::Fpga.as_u8(), 4);
        assert_eq!(LaneId::ALL.len(), 5);
    }

    #[test]
    fn test_from_u8() {
        assert_eq!(LaneId::from_u8(0), Some(LaneId::Cpu));
        assert_eq!(LaneId::from_u8(4), Some(LaneId::Fpga));
        assert_eq!(LaneId::from_u8(5), None);
    }

    #[test]
    fn test_algorithm_names() {
        assert_eq!(LaneId::Cpu.algorithm_name(), "RandomX");
        assert_eq!(LaneId::BtcAsic.algorithm_name(), "SHA-256");
    }
}
