use crate::constants::CREATE_DISCRIMINATOR_U64;
use solana_sdk::pubkey::Pubkey;

/// Zero-allocation view over a Pump.fun Create instruction payload
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInstructionView<'a> {
    pub name: &'a str,
    pub symbol: &'a str,
    pub uri: &'a str,
    pub creator: Option<Pubkey>,
}

impl<'a> CreateInstructionView<'a> {
    /// Parse the Create payload in nanoseconds without allocating Strings
    #[inline(always)]
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }

        let disc = unsafe { std::ptr::read_unaligned(data.as_ptr() as *const u64) };
        if disc != CREATE_DISCRIMINATOR_U64 {
            return None;
        }

        let mut cursor = &data[8..];

        let name = Self::read_str(&mut cursor)?;
        let symbol = Self::read_str(&mut cursor)?;
        let uri = Self::read_str(&mut cursor)?;

        let creator = if cursor.len() >= 32 {
            cursor[0..32].try_into().ok().map(Pubkey::new_from_array)
        } else {
            None
        };

        Some(Self {
            name,
            symbol,
            uri,
            creator,
        })
    }

    #[inline(always)]
    fn read_str(cursor: &mut &'a [u8]) -> Option<&'a str> {
        if cursor.len() < 4 {
            return None;
        }

        let len = unsafe {
            u32::from_le(std::ptr::read_unaligned(cursor.as_ptr() as *const u32))
        } as usize;
        *cursor = &cursor[4..];

        if cursor.len() < len {
            return None;
        }

        let slice = &cursor[..len];
        *cursor = &cursor[len..];

        // Validates UTF-8 in-place without heap allocations
        std::str::from_utf8(slice).ok()
    }
}
