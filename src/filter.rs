//! Implements filter used by both the 7z and XZ file format.

pub mod bcj;
pub mod bcj2;
pub mod delta;

/// Configuration for a filter in the XZ filter chain.
#[derive(Debug, Clone)]
pub struct FilterConfig {
    /// Filter type to use.
    pub filter_type: FilterType,
    /// Property to use.
    pub property: u32,
}

impl FilterConfig {
    /// Creates a new delta filter configuration.
    pub fn new_delta(distance: u32) -> Self {
        Self {
            filter_type: FilterType::Delta,
            property: distance,
        }
    }

    /// Creates a new BCJ x86 filter configuration.
    pub fn new_bcj_x86(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjX86,
            property: start_pos,
        }
    }

    /// Creates a new BCJ ARM filter configuration.
    pub fn new_bcj_arm(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjArm,
            property: start_pos,
        }
    }

    /// Creates a new BCJ ARM Thumb filter configuration.
    pub fn new_bcj_arm_thumb(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjArmThumb,
            property: start_pos,
        }
    }

    /// Creates a new BCJ ARM64 filter configuration.
    pub fn new_bcj_arm64(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjArm64,
            property: start_pos,
        }
    }

    /// Creates a new BCJ IA64 filter configuration.
    pub fn new_bcj_ia64(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjIa64,
            property: start_pos,
        }
    }

    /// Creates a new BCJ PPC filter configuration.
    pub fn new_bcj_ppc(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjPpc,
            property: start_pos,
        }
    }

    /// Creates a new BCJ SPARC filter configuration.
    pub fn new_bcj_sparc(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjSparc,
            property: start_pos,
        }
    }

    /// Creates a new BCJ RISC-V filter configuration.
    pub fn new_bcj_risc_v(start_pos: u32) -> Self {
        Self {
            filter_type: FilterType::BcjRiscv,
            property: start_pos,
        }
    }
}

/// Supported filter types in XZ format.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum FilterType {
    /// Delta filter
    Delta,
    /// BCJ x86 filter
    BcjX86,
    /// BCJ PowerPC filter
    BcjPpc,
    /// BCJ IA64 filter
    BcjIa64,
    /// BCJ ARM filter
    BcjArm,
    /// BCJ ARM Thumb
    BcjArmThumb,
    /// BCJ SPARC filter
    BcjSparc,
    /// BCJ ARM64 filter
    BcjArm64,
    /// BCJ RISC-V filter
    BcjRiscv,
    /// LZMA2 filter
    Lzma2,
}

impl TryFrom<u64> for FilterType {
    type Error = ();

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0x03 => Ok(FilterType::Delta),
            0x04 => Ok(FilterType::BcjX86),
            0x05 => Ok(FilterType::BcjPpc),
            0x06 => Ok(FilterType::BcjIa64),
            0x07 => Ok(FilterType::BcjArm),
            0x08 => Ok(FilterType::BcjArmThumb),
            0x09 => Ok(FilterType::BcjSparc),
            0x0A => Ok(FilterType::BcjArm64),
            0x0B => Ok(FilterType::BcjRiscv),
            0x21 => Ok(FilterType::Lzma2),
            _ => Err(()),
        }
    }
}
