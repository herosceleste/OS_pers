//! Tests hôte de l'isolation FPU/SSE/AVX par tâche (K004-06) : choix de la
//! stratégie de sauvegarde, zone de sauvegarde, état initial et motifs SIMD.

use std::collections::HashSet;

use elyon_core::fpu::{
    self, AreaLayout, CpuFeatures, FpuError, MXCSR_DEFAULT, SIMD_REGISTERS, SimdFile, Strategy,
    XCR0_AVX, XCR0_SSE, XCR0_X87,
};

fn features() -> CpuFeatures {
    CpuFeatures {
        fxsr: true,
        sse: true,
        sse2: true,
        xsave: true,
        avx: true,
        xcr0_supported: XCR0_X87 | XCR0_SSE | XCR0_AVX,
    }
}

// --- Choix de la stratégie --------------------------------------------------------

#[test]
fn xsave_with_avx_enables_x87_sse_and_avx() {
    assert_eq!(
        fpu::choose(&features()),
        Ok(Strategy::Xsave {
            xcr0: XCR0_X87 | XCR0_SSE | XCR0_AVX
        })
    );
}

#[test]
fn xsave_without_avx_enables_x87_and_sse_only() {
    let mut f = features();
    f.avx = false;
    assert_eq!(
        fpu::choose(&f),
        Ok(Strategy::Xsave {
            xcr0: XCR0_X87 | XCR0_SSE
        })
    );
}

#[test]
fn avx_is_not_enabled_unless_xsave_reports_it() {
    let mut f = features();
    f.xcr0_supported = XCR0_X87 | XCR0_SSE;
    assert_eq!(
        fpu::choose(&f),
        Ok(Strategy::Xsave {
            xcr0: XCR0_X87 | XCR0_SSE
        })
    );
}

#[test]
fn extra_components_like_avx512_are_never_enabled() {
    let mut f = features();
    f.xcr0_supported = 0xE7; // x87, SSE, AVX, opmask, ZMM_Hi256, Hi16_ZMM
    assert_eq!(
        fpu::choose(&f),
        Ok(Strategy::Xsave {
            xcr0: XCR0_X87 | XCR0_SSE | XCR0_AVX
        })
    );
}

#[test]
fn fxsave_is_the_fallback_without_xsave() {
    let mut f = features();
    f.xsave = false;
    assert_eq!(fpu::choose(&f), Ok(Strategy::Fxsave));
}

#[test]
fn fxsave_is_the_fallback_when_xsave_lacks_x87_or_sse() {
    let mut f = features();
    f.xcr0_supported = XCR0_X87;
    assert_eq!(fpu::choose(&f), Ok(Strategy::Fxsave));
}

#[test]
fn cpu_without_fxsr_or_sse2_is_rejected() {
    let mut no_fxsr = features();
    no_fxsr.fxsr = false;
    let mut no_sse2 = features();
    no_sse2.sse2 = false;
    assert_eq!(fpu::choose(&no_fxsr), Err(FpuError::NoFxsr));
    assert_eq!(fpu::choose(&no_sse2), Err(FpuError::NoSse));
}

// --- Zone de sauvegarde -----------------------------------------------------------

#[test]
fn fxsave_area_is_512_bytes_aligned_on_16() {
    assert_eq!(
        fpu::area_layout(Strategy::Fxsave, 0),
        Ok(AreaLayout {
            size: 512,
            align: 16
        })
    );
}

#[test]
fn xsave_area_uses_the_cpuid_size_aligned_on_64() {
    let strategy = Strategy::Xsave {
        xcr0: XCR0_X87 | XCR0_SSE | XCR0_AVX,
    };
    assert_eq!(
        fpu::area_layout(strategy, 832),
        Ok(AreaLayout {
            size: 832,
            align: 64
        })
    );
}

#[test]
fn xsave_area_smaller_than_legacy_plus_header_is_rejected() {
    let strategy = Strategy::Xsave {
        xcr0: XCR0_X87 | XCR0_SSE,
    };
    assert_eq!(fpu::area_layout(strategy, 512), Err(FpuError::AreaTooSmall));
}

#[test]
fn xsave_area_larger_than_one_page_is_rejected() {
    let strategy = Strategy::Xsave {
        xcr0: XCR0_X87 | XCR0_SSE,
    };
    assert_eq!(
        fpu::area_layout(strategy, 4097),
        Err(FpuError::AreaTooLarge)
    );
}

#[test]
fn area_address_must_respect_the_layout_alignment() {
    let layout = AreaLayout {
        size: 832,
        align: 64,
    };
    assert_eq!(fpu::validate_area(0x0020_0000, layout), Ok(()));
    assert_eq!(
        fpu::validate_area(0x0020_0020, layout),
        Err(FpuError::Misaligned)
    );
    assert_eq!(fpu::validate_area(0, layout), Err(FpuError::Misaligned));
}

// --- État initial -----------------------------------------------------------------

#[test]
fn init_image_has_default_control_words_and_empty_header() {
    let mut area = [0xFFu8; 832];
    fpu::write_init_image(&mut area);
    assert_eq!(u16::from_le_bytes([area[0], area[1]]), 0x037F);
    assert_eq!(
        u32::from_le_bytes([area[24], area[25], area[26], area[27]]),
        MXCSR_DEFAULT
    );
    // Reste de la zone legacy et en-tête XSAVE (XSTATE_BV, XCOMP_BV) à zéro.
    assert!(area[2..24].iter().all(|&byte| byte == 0));
    assert!(area[28..].iter().all(|&byte| byte == 0));
}

// --- Motifs SIMD ------------------------------------------------------------------

#[test]
fn xmm_and_ymm_patterns_are_unique_across_tasks_and_registers() {
    let mut seen = HashSet::new();
    for task in 0..3u8 {
        for reg in 0..SIMD_REGISTERS {
            assert!(seen.insert(fpu::xmm_pattern(task, reg)));
            assert!(seen.insert(fpu::ymm_high_pattern(task, reg)));
        }
    }
    assert_eq!(seen.len(), 3 * SIMD_REGISTERS * 2);
}

#[test]
fn every_pattern_half_uses_the_full_64_bit_width() {
    for task in 0..3u8 {
        for reg in 0..SIMD_REGISTERS {
            for value in [
                fpu::xmm_pattern(task, reg),
                fpu::ymm_high_pattern(task, reg),
            ] {
                assert!((value as u64) > u64::from(u32::MAX));
                assert!(((value >> 64) as u64) > u64::from(u32::MAX));
            }
        }
    }
}

#[test]
fn mxcsr_patterns_differ_per_task_and_keep_exceptions_masked() {
    let values: HashSet<_> = (0..3u8).map(fpu::mxcsr_pattern).collect();
    assert_eq!(values.len(), 3);
    for task in 0..3u8 {
        let mxcsr = fpu::mxcsr_pattern(task);
        assert_eq!(mxcsr & 0x1F80, 0x1F80, "exceptions masked");
        assert_eq!(mxcsr & 0x3F, 0, "no exception flag");
        assert_eq!(mxcsr & !0xFFFF, 0, "reserved bits clear");
    }
}

#[test]
fn expected_file_without_avx_has_no_upper_halves() {
    let file = fpu::expected_simd(1, false);
    assert!(file.ymm_high.iter().all(|&half| half == 0));
    assert_eq!(file.xmm[5], fpu::xmm_pattern(1, 5));
    assert_eq!(file.mxcsr, fpu::mxcsr_pattern(1));
}

#[test]
fn identical_simd_file_is_clean() {
    let expected = fpu::expected_simd(0, true);
    assert!(fpu::compare_simd(&expected, &expected, true).is_clean());
}

#[test]
fn corrupted_xmm_ymm_and_mxcsr_are_reported() {
    let expected = fpu::expected_simd(2, true);
    let mut observed: SimdFile = expected;
    observed.xmm[3] ^= 1;
    observed.ymm_high[15] = fpu::ymm_high_pattern(0, 15);
    observed.mxcsr = MXCSR_DEFAULT;
    let report = fpu::compare_simd(&expected, &observed, true);
    assert_eq!(report.xmm_mask, 1 << 3);
    assert_eq!(report.ymm_mask, 1 << 15);
    assert!(report.mxcsr_mismatch);
    assert_eq!(report.first_corrupted_xmm(), Some(3));
}

#[test]
fn upper_halves_are_ignored_without_avx() {
    let expected = fpu::expected_simd(0, false);
    let mut observed = expected;
    observed.ymm_high[0] = 0xDEAD;
    assert!(fpu::compare_simd(&expected, &observed, false).is_clean());
}
