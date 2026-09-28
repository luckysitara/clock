use solana_sdk::pubkey::Pubkey;

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
use std::arch::x86_64::*;

pub struct FastPubkeyFilter {
    target_pubkeys: Vec<[u8; 32]>,
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    simd_targets: Vec<__m256i>,
}

impl FastPubkeyFilter {
    pub fn new(pubkeys: &[Pubkey]) -> Self {
        let target_pubkeys: Vec<[u8; 32]> = pubkeys.iter().map(|p| p.to_bytes()).collect();

        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            let simd_targets = target_pubkeys
                .iter()
                .map(|bytes| unsafe {
                    _mm256_loadu_si256(bytes.as_ptr() as *const __m256i)
                })
                .collect();

            Self {
                target_pubkeys,
                simd_targets,
            }
        }

        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            Self { target_pubkeys }
        }
    }

    /// Fast scan checking if any pubkey in the account keys array matches our targets
    #[inline(always)]
    pub fn matches_any(&self, account_keys: &[[u8; 32]]) -> Option<Pubkey> {
        #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
        {
            unsafe {
                for key in account_keys {
                    let key_vec = _mm256_loadu_si256(key.as_ptr() as *const __m256i);
                    for (idx, target) in self.simd_targets.iter().enumerate() {
                        let cmp = _mm256_cmpeq_epi8(key_vec, *target);
                        let mask = _mm256_movemask_epi8(cmp);
                        if mask == -1 {
                            return Some(Pubkey::new_from_array(self.target_pubkeys[idx]));
                        }
                    }
                }
            }
            None
        }

        #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
        {
            for key in account_keys {
                for target in &self.target_pubkeys {
                    if key == target {
                        return Some(Pubkey::new_from_array(*target));
                    }
                }
            }
            None
        }
    }
}
