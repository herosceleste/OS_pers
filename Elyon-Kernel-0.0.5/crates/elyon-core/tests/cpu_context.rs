//! Tests hôte du contexte CPU préservé par le scheduler (K004-05).

use std::collections::HashSet;

use elyon_core::cpu_context::{self, GPR_COUNT, Gpr, RFLAGS_DF, RegisterFile, SwitchPath};

// --- Registres généraux -----------------------------------------------------------

#[test]
fn fifteen_general_purpose_registers_besides_rsp() {
    assert_eq!(GPR_COUNT, 15);
    assert_eq!(Gpr::ALL.len(), GPR_COUNT);
    let names: HashSet<_> = Gpr::ALL.iter().map(|gpr| gpr.name()).collect();
    assert_eq!(names.len(), GPR_COUNT);
    assert!(!names.contains("rsp"));
}

#[test]
fn register_order_matches_the_observed_buffer_layout() {
    // L'ordre sert d'index dans le buffer écrit par la boucle assembleur.
    assert_eq!(Gpr::ALL[0], Gpr::Rax);
    assert_eq!(Gpr::ALL[6], Gpr::Rbp);
    assert_eq!(Gpr::ALL[14], Gpr::R15);
    for (index, gpr) in Gpr::ALL.iter().enumerate() {
        assert_eq!(gpr.index(), index);
    }
}

#[test]
fn preemption_preserves_every_register() {
    for gpr in Gpr::ALL {
        assert!(
            cpu_context::preserved(SwitchPath::Preemption, gpr),
            "{}",
            gpr.name()
        );
    }
}

#[test]
fn cooperative_switch_preserves_only_callee_saved_registers() {
    let preserved: Vec<_> = Gpr::ALL
        .iter()
        .filter(|&&gpr| cpu_context::preserved(SwitchPath::Cooperative, gpr))
        .map(|gpr| gpr.name())
        .collect();
    assert_eq!(preserved, ["rbx", "rbp", "r12", "r13", "r14", "r15"]);
}

// --- Motifs -----------------------------------------------------------------------

#[test]
fn patterns_are_unique_across_tasks_and_registers() {
    let mut seen = HashSet::new();
    for task in 0..3u8 {
        for gpr in Gpr::ALL {
            assert!(seen.insert(cpu_context::pattern(task, gpr)));
        }
    }
    assert_eq!(seen.len(), 3 * GPR_COUNT);
}

#[test]
fn patterns_use_the_full_64_bit_width() {
    for task in 0..3u8 {
        for gpr in Gpr::ALL {
            let value = cpu_context::pattern(task, gpr);
            assert!(value > u64::from(u32::MAX), "{:#x}", value);
            assert_ne!(value, u64::MAX);
        }
    }
}

#[test]
fn expected_file_has_the_task_patterns_and_df_set() {
    let file = cpu_context::expected(1);
    for gpr in Gpr::ALL {
        assert_eq!(file.gprs[gpr.index()], cpu_context::pattern(1, gpr));
    }
    assert_ne!(file.rflags & RFLAGS_DF, 0);
}

// --- Détection de corruption ------------------------------------------------------

#[test]
fn identical_register_file_is_clean() {
    let expected = cpu_context::expected(0);
    let report = cpu_context::compare(&expected, &expected);
    assert!(report.is_clean());
    assert_eq!(report.first_corrupted(), None);
}

#[test]
fn a_single_corrupted_register_is_reported() {
    let expected = cpu_context::expected(0);
    let mut observed = expected;
    observed.gprs[Gpr::R9.index()] ^= 1;
    let report = cpu_context::compare(&expected, &observed);
    assert!(!report.is_clean());
    assert_eq!(report.corrupted_mask, 1 << Gpr::R9.index());
    assert_eq!(report.first_corrupted(), Some(Gpr::R9));
    assert!(!report.df_lost);
}

#[test]
fn a_register_leaked_from_another_task_is_detected() {
    let expected = cpu_context::expected(0);
    let mut observed = expected;
    observed.gprs[Gpr::Rax.index()] = cpu_context::pattern(2, Gpr::Rax);
    let report = cpu_context::compare(&expected, &observed);
    assert_eq!(report.first_corrupted(), Some(Gpr::Rax));
}

#[test]
fn several_corrupted_registers_are_all_reported() {
    let expected = cpu_context::expected(1);
    let mut observed = expected;
    observed.gprs[Gpr::Rcx.index()] = 0;
    observed.gprs[Gpr::R15.index()] = 0;
    let report = cpu_context::compare(&expected, &observed);
    assert_eq!(
        report.corrupted_mask,
        (1 << Gpr::Rcx.index()) | (1 << Gpr::R15.index())
    );
    assert_eq!(report.first_corrupted(), Some(Gpr::Rcx));
}

#[test]
fn lost_direction_flag_is_detected() {
    let expected = cpu_context::expected(2);
    let mut observed = expected;
    observed.rflags &= !RFLAGS_DF;
    let report = cpu_context::compare(&expected, &observed);
    assert!(report.df_lost);
    assert!(!report.is_clean());
    assert_eq!(report.corrupted_mask, 0);
}

#[test]
fn other_rflags_bits_are_not_compared() {
    // CF, ZF, IF… évoluent légitimement entre deux observations.
    let expected = cpu_context::expected(2);
    let mut observed: RegisterFile = expected;
    observed.rflags |= 0x1 | 0x40 | 0x200;
    assert!(cpu_context::compare(&expected, &observed).is_clean());
}
