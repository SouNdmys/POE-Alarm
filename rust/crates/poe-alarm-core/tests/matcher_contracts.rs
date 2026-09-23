use poe_alarm_core::{Decimal, FullLineAffixMatcher, canonicalize, extract_values};

#[test]
fn a_fixed_value_parenthesis_is_metadata_for_one_observed_slot() {
    for (template, text, actual) in [
        (
            "+# to Level of all Attack Skills",
            "+4(3) to Level of all Attack Skills",
            Decimal::from(4),
        ),
        (
            "#% increased Attack Speed",
            "22(25)% increased Attack Speed",
            Decimal::from(22),
        ),
        (
            "-#% to Fire Resistance",
            "-4(3)% to Fire Resistance",
            Decimal::from(-4),
        ),
        (
            "+#% to Critical Hit Chance",
            "+4.78(4.5)% to Critical Hit Chance",
            "4.78".parse().unwrap(),
        ),
    ] {
        let matcher = FullLineAffixMatcher::new(template).unwrap();
        assert!(matcher.is_match(text), "{text}");
        assert_eq!(extract_values(text), vec![Some(actual)]);
        // Pasting a captured line as the template must also yield one slot.
        assert_eq!(canonicalize(text), canonicalize(template));
    }
}

#[test]
fn fixed_value_metadata_does_not_weaken_units_signs_or_swallow_explanations() {
    let matcher = FullLineAffixMatcher::new("+# to Level of all Attack Skills").unwrap();
    for text in [
        "-4(3) to Level of all Attack Skills",
        "+4(3)% to Level of all Attack Skills",
        "+4(3 to Level of all Attack Skills",
        "+4(3 charges) to Level of all Attack Skills",
        "+4(3%) to Level of all Attack Skills",
        "+4 (3) to Level of all Attack Skills",
        "+4(3) to Level of all Melee Skills",
    ] {
        assert!(!matcher.is_match(text), "{text}");
    }
}

#[test]
fn english_full_line_contract_rejects_near_neighbours() {
    let matcher = FullLineAffixMatcher::new("#% increased Attack Speed").unwrap();
    assert!(matcher.is_match("27% increased Attack Speed"));
    assert!(!matcher.is_match("27% increased Cast Speed"));
    assert!(!matcher.is_match("27% increased Attack Speed Recently"));
    assert!(!matcher.is_match("prefix 27% increased Attack Speed"));
    assert!(!matcher.is_match("27 increased Attack Speed"));
}

#[test]
fn sign_and_numeric_slot_structure_remain_strict() {
    let resistance = FullLineAffixMatcher::new("+#% to Fire Resistance").unwrap();
    assert!(resistance.is_match("35% to Fire Resistance"));
    assert!(!resistance.is_match("-35% to Fire Resistance"));

    let armour = FullLineAffixMatcher::new("+# to Armour and # Life per second").unwrap();
    assert!(armour.is_match("+159 to Armour and 3.5 Life per second"));
    assert_eq!(
        extract_values("+159 to Armour and 3.5 Life per second"),
        vec![Some(Decimal::from(159)), Some(Decimal::from(3.5))]
    );
}

#[test]
fn only_a_range_spanning_zero_accepts_both_signs_and_keeps_units_strict() {
    for template in [
        "Breaches in Map have (-10—20)% reduced Pack Size",
        "Breaches in Map have -10-20% reduced Pack Size",
        "Breaches in Map have (-10—0)% reduced Pack Size",
    ] {
        let matcher = FullLineAffixMatcher::new(template).unwrap();
        for value in [-10, -1, 0, 1, 20] {
            assert!(
                matcher.is_match(&format!("Breaches in Map have {value}% reduced Pack Size")),
                "{template}: {value}"
            );
        }
        assert!(!matcher.is_match("Breaches in Map have 10 reduced Pack Size"));
        assert!(!matcher.is_match("Breaches in Map have 10% increased Pack Size"));
    }
    let flat = FullLineAffixMatcher::new("(-10—20) to maximum Life").unwrap();
    assert!(flat.is_match("-1 to maximum Life"));
    assert!(flat.is_match("0 to maximum Life"));
    assert!(flat.is_match("20 to maximum Life"));
    assert!(!flat.is_match("20% to maximum Life"));

    for template in ["-#%", "(-20—-10)%", "-15(-20—-10)%"] {
        let matcher = FullLineAffixMatcher::new(format!("{template} to Fire Resistance")).unwrap();
        assert!(matcher.is_match("-15% to Fire Resistance"));
        assert!(!matcher.is_match("0% to Fire Resistance"));
        assert!(!matcher.is_match("15% to Fire Resistance"));
        // A displayed roll's sign wins over the trailing tier range.
        assert!(!matcher.is_match("15(-10-20)% to Fire Resistance"));
    }
    for template in ["#%", "+#%", "(0—20)%", "(10—20)%"] {
        let matcher = FullLineAffixMatcher::new(format!("{template} to Fire Resistance")).unwrap();
        assert!(!matcher.is_match("-15% to Fire Resistance"));
        assert!(matcher.is_match("15% to Fire Resistance"));
    }
}

#[test]
fn full_width_and_ocr_glyph_confusions_are_narrowly_recovered() {
    let matcher = FullLineAffixMatcher::new("#% increased attack speed").unwrap();
    assert!(matcher.is_match("８％ ＩＮＣＲＥＡＳＥＤ ＡＴＴＡＣＫ ＳＰＥＥＤ"));

    assert!(
        FullLineAffixMatcher::new("+# to maximum Life")
            .unwrap()
            .is_match("+70 to maximum L1fe")
    );
    assert!(
        !FullLineAffixMatcher::new("+# to maximum Life")
            .unwrap()
            .is_match("+70 to minimum L1fe")
    );
}

#[test]
fn traditional_chinese_spacing_is_presentation_only() {
    let matcher = FullLineAffixMatcher::new("若你近期有暴擊，增加 (6—8)% 攻擊速度").unwrap();
    assert!(matcher.is_match("若 你 近 期 有 暴 擊，增 加 8% 攻 擊 速 度"));
    assert!(!matcher.is_match("若你近期有擊殺，增加8%攻擊速度"));
    assert!(!matcher.is_match("若你近期有暴擊，增加8%施放速度"));
    assert_eq!(canonicalize("+(3.11—3.8)%暴擊率").text, "<PCT> 暴 擊 率");
}

#[test]
fn all_supported_numeric_renderings_have_the_same_shape() {
    let matcher =
        FullLineAffixMatcher::new("(4—5) to (9—10) Added Cold Damage with Dagger Attacks").unwrap();
    assert!(matcher.is_match("# to # Added Cold Damage with Dagger Attacks"));
    assert!(matcher.is_match("5 to 10 Added Cold Damage with Dagger Attacks"));
    assert!(matcher.is_match("5(4-5) to 10(9-10) Added Cold Damage with Dagger Attacks"));
    assert!(!matcher.is_match("10 Added Cold Damage with Dagger Attacks"));
}

#[test]
fn presentation_and_wrapping_do_not_change_semantics() {
    let matcher = FullLineAffixMatcher::new(
        "(6-8)% INCREASED ATTACK SPEED IF YOU’VE DEALT A CRITICAL STRIKE RECENTLY",
    )
    .unwrap();
    assert!(
        matcher.is_match(
            "  8%  increased Attack Speed\r\nif you've dealt a Critical Strike Recently. "
        )
    );

    let lines = vec![
        "Adds 10 to 17 Cold Damage to Attacks".into(),
        "8(6-8)% increased Attack Speed if you've dealt".into(),
        "a Critical Strike Recently".into(),
    ];
    let found = matcher.find_match(&lines).unwrap();
    assert_eq!(found.start_line_index, 1);
    assert_eq!(found.physical_line_count, 2);
}

#[test]
fn poe2_literals_are_numeric_slots_and_eight_lines_are_supported() {
    let projectiles = FullLineAffixMatcher::new("Monsters fire 2 additional Projectiles").unwrap();
    assert!(projectiles.is_match("Monsters fire 3 additional Projectiles"));

    let matcher = FullLineAffixMatcher::with_maximum_line_span(
        "Monsters deal increased Damage and fire additional Projectiles while the Area contains dangerous terrain",
        8,
    )
    .unwrap();
    let lines = [
        "Monsters deal",
        "increased Damage",
        "and fire",
        "additional Projectiles",
        "while the Area",
        "contains",
        "dangerous",
        "terrain",
    ]
    .map(str::to_owned);
    assert_eq!(matcher.find_match(&lines).unwrap().physical_line_count, 8);
    assert!(FullLineAffixMatcher::with_maximum_line_span("#% increased Attack Speed", 9).is_err());
}

#[test]
fn semantic_neighbours_are_never_fuzzy_matches() {
    for (expected, other) in [
        ("Attack", "Cast"),
        ("Dagger", "Claw"),
        ("Cold", "Fire"),
        ("dealt", "killed"),
        ("you've", "haven't"),
        ("increased", "reduced"),
    ] {
        let matcher = FullLineAffixMatcher::new(format!("#% {expected} speed recently")).unwrap();
        assert!(!matcher.is_match(&format!("8% {other} speed recently")));
    }
}

#[test]
fn in_word_hyphen_loss_is_recovered() {
    let matcher =
        FullLineAffixMatcher::new("#% reduced Mana Cost of Non-Channelling Skills").unwrap();
    assert!(matcher.is_match("7% reduced Mana Cost of NonChannelling Skills"));
}

#[test]
fn a_value_annotation_tail_never_blocks_a_match() {
    // The client appends ` — Unscalable Value` to some rolled lines. The tail
    // is not part of the modifier and must not cost the user the hit.
    let speed =
        FullLineAffixMatcher::new("(25—30)% increased Explicit Speed Modifier magnitudes").unwrap();
    assert!(
        speed
            .is_match("30(25-30)% increased Explicit Speed Modifier magnitudes — Unscalable Value")
    );

    let tc = FullLineAffixMatcher::new("增加(25—30)%變動速度詞綴的大小").unwrap();
    assert!(tc.is_match("增加30(25-30)%變動速度詞綴的大小 — 無法變動的值"));

    // POE1's client ships a different translation of the same tail, which is
    // why the strip is structural rather than a word list.
    let socket = FullLineAffixMatcher::new("有 1 個深淵插槽").unwrap();
    assert!(socket.is_match("有 1 個深淵插槽 — 無法使用的值"));
}

#[test]
fn a_template_copied_with_the_tail_still_matches() {
    // A user who builds the rule by copying the whole line from the item gets
    // the same behaviour as one who copies the clean PoEDB template.
    let copied = FullLineAffixMatcher::new(
        "30(25-30)% increased Explicit Speed Modifier magnitudes — Unscalable Value",
    )
    .unwrap();
    assert!(copied.is_match("28% increased Explicit Speed Modifier magnitudes"));
}

#[test]
fn a_dash_tail_carrying_values_is_modifier_text_not_annotation() {
    let matcher = FullLineAffixMatcher::new("增加 (40—50)% 冰冷傷害").unwrap();
    // Numeric content after the dash means real modifier text: stripping it
    // would let this near-neighbour line satisfy the rule.
    assert!(!matcher.is_match("增加 50% 冰冷傷害 — 增加 10% 攻擊速度"));
    // And a range dash inside parentheses never opens a tail.
    assert!(matcher.is_match("增加 50(40-50)% 冰冷傷害"));
}

#[test]
fn the_tail_is_stripped_before_lines_join_into_a_span() {
    // Once lines merge, the tail sits mid-string and is no longer trailing;
    // the strip has to happen per line, before the join.
    let hybrid = FullLineAffixMatcher::new("增加 (75—79)% 物理傷害 +(175—200) 命中值").unwrap();
    let lines = vec![
        "增加 79(75-79)% 物理傷害 — 無法變動的值".to_string(),
        "+186(175-200) 命中值".to_string(),
    ];
    let found = hybrid.find_match(&lines).expect("hybrid still matches");
    assert_eq!(found.physical_line_count, 2);
}
