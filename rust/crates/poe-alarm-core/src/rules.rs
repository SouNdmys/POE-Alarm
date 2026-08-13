use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::Decimal;
use crate::matching::{
    CanonicalAffix, DEFAULT_MAXIMUM_PHYSICAL_LINE_SPAN, FullLineAffixMatcher,
    MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN, canonicalize, extract_values,
};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const MAXIMUM_GROUPS: usize = 8;
pub const MAXIMUM_CONDITIONS: usize = 32;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum NumericConstraintMode {
    #[default]
    Ignore,
    RangeInclusive,
    AtLeast,
    AtMost,
    Exactly,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NumericConstraint {
    #[serde(default)]
    pub mode: NumericConstraintMode,
    #[serde(default)]
    pub minimum: Option<Decimal>,
    #[serde(default)]
    pub maximum: Option<Decimal>,
    #[serde(default)]
    pub expected: Option<Decimal>,
}

impl NumericConstraint {
    pub fn ignored() -> Self {
        Self::default()
    }

    pub fn range(minimum: impl Into<Decimal>, maximum: impl Into<Decimal>) -> Self {
        Self {
            mode: NumericConstraintMode::RangeInclusive,
            minimum: Some(minimum.into()),
            maximum: Some(maximum.into()),
            expected: None,
        }
    }

    pub fn at_least(minimum: impl Into<Decimal>) -> Self {
        Self {
            mode: NumericConstraintMode::AtLeast,
            minimum: Some(minimum.into()),
            maximum: None,
            expected: None,
        }
    }

    pub fn at_most(maximum: impl Into<Decimal>) -> Self {
        Self {
            mode: NumericConstraintMode::AtMost,
            minimum: None,
            maximum: Some(maximum.into()),
            expected: None,
        }
    }

    pub fn exactly(expected: impl Into<Decimal>) -> Self {
        Self {
            mode: NumericConstraintMode::Exactly,
            minimum: None,
            maximum: None,
            expected: Some(expected.into()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AffixCondition {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub numeric_constraints: Vec<NumericConstraint>,
}

impl AffixCondition {
    pub fn new(
        name: impl Into<String>,
        template: impl Into<String>,
        numeric_constraints: Vec<NumericConstraint>,
    ) -> Self {
        Self {
            name: name.into(),
            template: template.into(),
            numeric_constraints,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum ResultGroupMode {
    #[default]
    Any,
    All,
    AtLeast,
}

fn default_required_count() -> usize {
    1
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptableResultGroup {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mode: ResultGroupMode,
    #[serde(default = "default_required_count")]
    pub required_count: usize,
    #[serde(default)]
    pub conditions: Vec<AffixCondition>,
}

impl Default for AcceptableResultGroup {
    fn default() -> Self {
        Self {
            name: String::new(),
            mode: ResultGroupMode::Any,
            required_count: 1,
            conditions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSetDefinition {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub groups: Vec<AcceptableResultGroup>,
}

const fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

impl Default for RuleSetDefinition {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            name: String::new(),
            groups: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalLineIdentity {
    pub line_index: usize,
    pub physical_band_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssistedModifierObservation {
    pub physical_band_id: String,
    pub original_text: String,
    pub canonical_target: String,
    pub related_physical_band_ids: Vec<String>,
}

impl AssistedModifierObservation {
    pub fn new(
        physical_band_id: impl Into<String>,
        original_text: impl Into<String>,
        canonical_target: impl Into<String>,
    ) -> Self {
        Self {
            physical_band_id: physical_band_id.into(),
            original_text: original_text.into(),
            canonical_target: canonical_target.into(),
            related_physical_band_ids: Vec::new(),
        }
    }

    fn physical_band_ids(&self) -> impl Iterator<Item = &str> {
        self.related_physical_band_ids
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.physical_band_id.as_str()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModifierObservation {
    pub start_line_index: usize,
    pub physical_line_count: usize,
    pub original_text: String,
    pub canonical_text: String,
    pub numeric_values: Vec<Option<Decimal>>,
    pub physical_band_id: Option<String>,
    pub physical_band_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NumericSlotEvidence {
    pub slot_index: usize,
    pub actual_value: Option<Decimal>,
    pub constraint: NumericConstraint,
    pub is_satisfied: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConditionEvaluation {
    pub condition_index: usize,
    pub name: String,
    pub template: String,
    pub is_matched: bool,
    pub observation: Option<ModifierObservation>,
    pub numeric_slots: Vec<NumericSlotEvidence>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultGroupEvaluation {
    pub group_index: usize,
    pub name: String,
    pub mode: ResultGroupMode,
    pub required_count: usize,
    pub matched_count: usize,
    pub is_match: bool,
    pub conditions: Vec<ConditionEvaluation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleEvaluationResult {
    pub is_match: bool,
    pub matched_group_index: Option<usize>,
    pub groups: Vec<ResultGroupEvaluation>,
}

impl RuleEvaluationResult {
    pub fn matched_group(&self) -> Option<&ResultGroupEvaluation> {
        self.matched_group_index
            .and_then(|index| self.groups.get(index))
    }

    pub fn matched_group_id(&self) -> Option<&str> {
        self.matched_group().map(|group| group.name.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleValidationError {
    pub errors: Vec<String>,
}

impl std::fmt::Display for RuleValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.errors.join("\n"))
    }
}

impl std::error::Error for RuleValidationError {}

#[derive(Clone, Debug)]
pub struct CompiledRuleSet {
    definition: RuleSetDefinition,
    groups: Vec<CompiledGroup>,
    targets: Vec<FullLineAffixMatcher>,
    maximum_physical_line_span: usize,
}

impl CompiledRuleSet {
    pub fn compile(definition: RuleSetDefinition) -> Result<Self, RuleValidationError> {
        Self::compile_with_maximum_line_span(definition, DEFAULT_MAXIMUM_PHYSICAL_LINE_SPAN)
    }

    pub fn compile_with_maximum_line_span(
        definition: RuleSetDefinition,
        maximum_physical_line_span: usize,
    ) -> Result<Self, RuleValidationError> {
        validate_definition(&definition, maximum_physical_line_span)?;
        let groups = definition
            .groups
            .iter()
            .enumerate()
            .map(|(group_index, group)| {
                compile_group(group, group_index, maximum_physical_line_span)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut target_keys = HashSet::new();
        let mut targets = Vec::new();
        for matcher in groups
            .iter()
            .flat_map(|group| group.conditions.iter().map(|condition| &condition.matcher))
        {
            let key = (matcher.maximum_line_span(), matcher.template().text.clone());
            if target_keys.insert(key) {
                targets.push(matcher.clone());
            }
        }

        Ok(Self {
            definition,
            groups,
            targets,
            maximum_physical_line_span,
        })
    }

    pub fn definition(&self) -> &RuleSetDefinition {
        &self.definition
    }

    /// Deduplicated lexical targets for a target-aware OCR adapter.
    pub fn targets(&self) -> &[FullLineAffixMatcher] {
        &self.targets
    }

    pub fn evaluate(&self, physical_lines: &[String]) -> RuleEvaluationResult {
        self.evaluate_with_identity(physical_lines, &[], &[])
    }

    pub fn evaluate_with_identity(
        &self,
        physical_lines: &[String],
        assisted_observations: &[AssistedModifierObservation],
        physical_line_identities: &[PhysicalLineIdentity],
    ) -> RuleEvaluationResult {
        let observations = prepare_observations(
            physical_lines,
            assisted_observations,
            physical_line_identities,
            self.maximum_physical_line_span,
        );
        let mut groups = Vec::with_capacity(self.groups.len());
        let mut matched_group_index = None;
        for (index, group) in self.groups.iter().enumerate() {
            let evaluation = evaluate_group(group, index, &observations);
            if matched_group_index.is_none() && evaluation.is_match {
                matched_group_index = Some(index);
            }
            groups.push(evaluation);
        }
        RuleEvaluationResult {
            is_match: matched_group_index.is_some(),
            matched_group_index,
            groups,
        }
    }
}

#[derive(Clone, Debug)]
struct CompiledGroup {
    definition: AcceptableResultGroup,
    required_count: usize,
    conditions: Vec<CompiledCondition>,
}

#[derive(Clone, Debug)]
struct CompiledCondition {
    definition: AffixCondition,
    matcher: FullLineAffixMatcher,
    constraints: Vec<NumericConstraint>,
    constrained_slot_count: usize,
    constraint_specificity: usize,
    lexical_token_count: usize,
    original_index: usize,
}

fn validate_definition(
    definition: &RuleSetDefinition,
    maximum_line_span: usize,
) -> Result<(), RuleValidationError> {
    let mut errors = Vec::new();
    if !(1..=MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN).contains(&maximum_line_span) {
        errors.push(format!(
            "line span must be between 1 and {MAXIMUM_SUPPORTED_PHYSICAL_LINE_SPAN}"
        ));
    }
    if definition.schema_version != CURRENT_SCHEMA_VERSION {
        errors.push(format!(
            "unsupported rule schema version {}",
            definition.schema_version
        ));
    }
    if !(1..=MAXIMUM_GROUPS).contains(&definition.groups.len()) {
        errors.push(format!(
            "a rule set must contain 1-{MAXIMUM_GROUPS} acceptable results"
        ));
    }
    let total_conditions = definition
        .groups
        .iter()
        .map(|group| group.conditions.len())
        .sum::<usize>();
    if total_conditions > MAXIMUM_CONDITIONS {
        errors.push(format!(
            "a rule set may contain at most {MAXIMUM_CONDITIONS} conditions"
        ));
    }

    let mut group_names = HashSet::new();
    for (group_index, group) in definition.groups.iter().enumerate() {
        let name = group.name.trim();
        if !name.is_empty() && !group_names.insert(name.to_owned()) {
            errors.push(format!("acceptable result name '{name}' is duplicated"));
        }
        if group.conditions.is_empty() {
            errors.push(format!(
                "acceptable result {} has no conditions",
                group_index + 1
            ));
        }
        if group.mode == ResultGroupMode::AtLeast
            && !(1..=group.conditions.len()).contains(&group.required_count)
        {
            errors.push(format!(
                "acceptable result {} requires between 1 and {} conditions",
                group_index + 1,
                group.conditions.len()
            ));
        }
        let mut condition_names = HashSet::new();
        for (condition_index, condition) in group.conditions.iter().enumerate() {
            let name = condition.name.trim();
            if !name.is_empty() && !condition_names.insert(name.to_owned()) {
                errors.push(format!(
                    "acceptable result {} contains duplicate condition name '{name}'",
                    group_index + 1
                ));
            }
            if condition.template.trim().is_empty() {
                errors.push(format!(
                    "acceptable result {}, condition {} has an empty template",
                    group_index + 1,
                    condition_index + 1
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RuleValidationError { errors })
    }
}

fn compile_group(
    group: &AcceptableResultGroup,
    group_index: usize,
    maximum_line_span: usize,
) -> Result<CompiledGroup, RuleValidationError> {
    let required_count = match group.mode {
        ResultGroupMode::Any => 1,
        ResultGroupMode::All => group.conditions.len(),
        ResultGroupMode::AtLeast => group.required_count,
    };
    let conditions = group
        .conditions
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            compile_condition(condition, group_index, index, maximum_line_span)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CompiledGroup {
        definition: group.clone(),
        required_count,
        conditions,
    })
}

fn compile_condition(
    condition: &AffixCondition,
    group_index: usize,
    condition_index: usize,
    maximum_line_span: usize,
) -> Result<CompiledCondition, RuleValidationError> {
    let matcher =
        FullLineAffixMatcher::with_maximum_line_span(&condition.template, maximum_line_span)
            .map_err(|error| RuleValidationError {
                errors: vec![format!(
                    "acceptable result {}, condition {}: {error}",
                    group_index + 1,
                    condition_index + 1
                )],
            })?;
    let numeric_slot_count = matcher
        .template()
        .tokens
        .iter()
        .filter(|token| !token.kind.is_word())
        .count();
    if condition.numeric_constraints.len() > numeric_slot_count {
        return Err(RuleValidationError {
            errors: vec![format!(
                "acceptable result {}, condition {} defines {} numeric constraints for a {numeric_slot_count}-slot template",
                group_index + 1,
                condition_index + 1,
                condition.numeric_constraints.len()
            )],
        });
    }
    let mut constraints = Vec::with_capacity(numeric_slot_count);
    for slot in 0..numeric_slot_count {
        let constraint = condition
            .numeric_constraints
            .get(slot)
            .cloned()
            .unwrap_or_default();
        if !valid_constraint(&constraint) {
            return Err(RuleValidationError {
                errors: vec![format!(
                    "acceptable result {}, condition {}, numeric slot {} has an invalid {:?} constraint",
                    group_index + 1,
                    condition_index + 1,
                    slot + 1,
                    constraint.mode
                )],
            });
        }
        constraints.push(constraint);
    }
    let constrained_slot_count = constraints
        .iter()
        .filter(|constraint| constraint.mode != NumericConstraintMode::Ignore)
        .count();
    let constraint_specificity = constraints.iter().map(constraint_specificity).sum();
    let lexical_token_count = matcher
        .template()
        .tokens
        .iter()
        .filter(|token| token.kind.is_word())
        .count();
    Ok(CompiledCondition {
        definition: condition.clone(),
        matcher,
        constraints,
        constrained_slot_count,
        constraint_specificity,
        lexical_token_count,
        original_index: condition_index,
    })
}

fn valid_constraint(constraint: &NumericConstraint) -> bool {
    match constraint.mode {
        NumericConstraintMode::Ignore => true,
        NumericConstraintMode::RangeInclusive => matches!(
            (constraint.minimum, constraint.maximum),
            (Some(minimum), Some(maximum)) if minimum <= maximum
        ),
        NumericConstraintMode::AtLeast => constraint.minimum.is_some(),
        NumericConstraintMode::AtMost => constraint.maximum.is_some(),
        NumericConstraintMode::Exactly => constraint.expected.is_some(),
    }
}

fn constraint_specificity(constraint: &NumericConstraint) -> usize {
    match constraint.mode {
        NumericConstraintMode::Exactly => 4,
        NumericConstraintMode::RangeInclusive => 3,
        NumericConstraintMode::AtLeast | NumericConstraintMode::AtMost => 2,
        NumericConstraintMode::Ignore => 0,
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ObservationIdentity {
    start_line_index: usize,
    physical_line_count: usize,
    canonical_text: String,
    physical_identity_key: String,
}

#[derive(Clone, Debug)]
struct PreparedObservation {
    start_line_index: usize,
    physical_line_count: usize,
    original_text: String,
    canonical: CanonicalAffix,
    verified_canonical_target: Option<String>,
    physical_band_ids: Vec<String>,
    identity: ObservationIdentity,
}

impl PreparedObservation {
    fn new(
        start_line_index: usize,
        physical_line_count: usize,
        original_text: String,
        canonical: CanonicalAffix,
        mut physical_band_ids: Vec<String>,
        verified_canonical_target: Option<String>,
    ) -> Self {
        physical_band_ids.retain(|id| !id.trim().is_empty());
        for id in &mut physical_band_ids {
            *id = id.trim().to_owned();
        }
        physical_band_ids.sort();
        physical_band_ids.dedup();
        let verified_canonical_target = verified_canonical_target
            .filter(|target| !target.trim().is_empty())
            .map(|target| target.trim().to_owned());
        let identity = ObservationIdentity {
            start_line_index,
            physical_line_count,
            canonical_text: verified_canonical_target
                .clone()
                .unwrap_or_else(|| canonical.text.clone()),
            physical_identity_key: physical_band_ids.join("\n"),
        };
        Self {
            start_line_index,
            physical_line_count,
            original_text,
            canonical,
            verified_canonical_target,
            physical_band_ids,
            identity,
        }
    }

    fn to_observation(&self, matched_template: &CanonicalAffix) -> ModifierObservation {
        let mut numeric_values = extract_values(&self.original_text);
        if self.verified_canonical_target.is_some() {
            let expected_kinds: Vec<_> = matched_template
                .tokens
                .iter()
                .filter(|token| !token.kind.is_word())
                .map(|token| token.kind)
                .collect();
            let actual_kinds: Vec<_> = self
                .canonical
                .tokens
                .iter()
                .filter(|token| !token.kind.is_word())
                .map(|token| token.kind)
                .collect();
            if expected_kinds != actual_kinds {
                numeric_values = vec![None; expected_kinds.len()];
            }
        }
        ModifierObservation {
            start_line_index: self.start_line_index,
            physical_line_count: self.physical_line_count,
            original_text: self.original_text.clone(),
            canonical_text: self
                .verified_canonical_target
                .clone()
                .unwrap_or_else(|| self.canonical.text.clone()),
            numeric_values,
            physical_band_id: self.physical_band_ids.first().cloned(),
            physical_band_ids: self.physical_band_ids.clone(),
        }
    }
}

fn prepare_observations(
    physical_lines: &[String],
    assisted_observations: &[AssistedModifierObservation],
    physical_line_identities: &[PhysicalLineIdentity],
    maximum_line_span: usize,
) -> Vec<PreparedObservation> {
    let mut physical_band_by_line = HashMap::new();
    for identity in physical_line_identities {
        if identity.line_index < physical_lines.len() {
            physical_band_by_line
                .entry(identity.line_index)
                .or_insert_with(|| identity.physical_band_id.clone());
        }
    }

    let mut observations = Vec::with_capacity(physical_lines.len() * 2);
    for start in 0..physical_lines.len() {
        if physical_lines[start].trim().is_empty() {
            continue;
        }
        let mut combined = String::new();
        let maximum_span = maximum_line_span.min(physical_lines.len() - start);
        for span in 1..=maximum_span {
            let line = physical_lines[start + span - 1].trim();
            if line.is_empty() {
                break;
            }
            if !combined.is_empty() {
                combined.push(' ');
            }
            combined.push_str(line);
            let band_ids = (start..start + span)
                .filter_map(|line_index| physical_band_by_line.get(&line_index).cloned())
                .collect();
            observations.push(PreparedObservation::new(
                start,
                span,
                combined.clone(),
                canonicalize(&combined),
                band_ids,
                None,
            ));
        }
    }

    let mut assisted_index = 0;
    for assisted in assisted_observations {
        if assisted.original_text.trim().is_empty()
            || assisted.physical_band_id.trim().is_empty()
            || assisted.canonical_target.trim().is_empty()
        {
            continue;
        }
        let verified = canonicalize(&assisted.canonical_target).text;
        if verified.trim().is_empty() {
            continue;
        }
        observations.push(PreparedObservation::new(
            physical_lines.len() + assisted_index,
            1,
            assisted.original_text.trim().to_owned(),
            canonicalize(&assisted.original_text),
            assisted.physical_band_ids().map(str::to_owned).collect(),
            Some(verified),
        ));
        assisted_index += 1;
    }
    observations
}

#[derive(Clone, Debug)]
struct ConditionCandidate {
    identity: ObservationIdentity,
    observation: ModifierObservation,
    numeric_slots: Vec<NumericSlotEvidence>,
    is_numeric_match: bool,
}

#[derive(Clone, Debug)]
struct ComponentCandidate {
    component_id: usize,
    candidate: ConditionCandidate,
}

fn evaluate_group(
    group: &CompiledGroup,
    group_index: usize,
    observations: &[PreparedObservation],
) -> ResultGroupEvaluation {
    let candidates: Vec<Vec<ConditionCandidate>> = group
        .conditions
        .iter()
        .map(|condition| find_candidates(condition, observations))
        .collect();
    let mut condition_order: Vec<usize> = (0..group.conditions.len()).collect();
    condition_order.sort_by(|&left, &right| {
        let left = &group.conditions[left];
        let right = &group.conditions[right];
        right
            .constrained_slot_count
            .cmp(&left.constrained_slot_count)
            .then_with(|| {
                right
                    .constraint_specificity
                    .cmp(&left.constraint_specificity)
            })
            .then_with(|| right.lexical_token_count.cmp(&left.lexical_token_count))
            .then_with(|| left.original_index.cmp(&right.original_index))
    });
    let components = build_observation_components(&candidates);
    let component_candidates = build_component_candidates(&candidates, &components);
    let assigned = find_maximum_component_assignment(&component_candidates, &condition_order);

    let mut matched_count = 0;
    let mut conditions = Vec::with_capacity(group.conditions.len());
    for (index, compiled) in group.conditions.iter().enumerate() {
        let assigned_candidate = assigned[index].clone();
        let best = assigned_candidate
            .clone()
            .or_else(|| best_text_candidate(&candidates[index]).cloned());
        if assigned_candidate.is_some() {
            matched_count += 1;
        }
        conditions.push(ConditionEvaluation {
            condition_index: index,
            name: if compiled.definition.name.trim().is_empty() {
                compiled.definition.template.clone()
            } else {
                compiled.definition.name.clone()
            },
            template: compiled.definition.template.clone(),
            is_matched: assigned_candidate.is_some(),
            observation: best.as_ref().map(|candidate| candidate.observation.clone()),
            numeric_slots: best
                .map(|candidate| candidate.numeric_slots)
                .unwrap_or_default(),
        });
    }

    ResultGroupEvaluation {
        group_index,
        name: if group.definition.name.trim().is_empty() {
            format!("Result {}", group_index + 1)
        } else {
            group.definition.name.clone()
        },
        mode: group.definition.mode,
        required_count: group.required_count,
        matched_count,
        is_match: matched_count >= group.required_count,
        conditions,
    }
}

fn find_candidates(
    condition: &CompiledCondition,
    observations: &[PreparedObservation],
) -> Vec<ConditionCandidate> {
    let mut candidates = Vec::new();
    for prepared in observations {
        let text_matches = match &prepared.verified_canonical_target {
            Some(target) => condition.matcher.template().text == *target,
            None => condition.matcher.is_canonical_match(&prepared.canonical),
        };
        if prepared.physical_line_count > condition.matcher.maximum_line_span() || !text_matches {
            continue;
        }
        let observation = prepared.to_observation(condition.matcher.template());
        let numeric_slots =
            evaluate_numeric_slots(&condition.constraints, &observation.numeric_values);
        let is_numeric_match = numeric_slots.iter().all(|evidence| evidence.is_satisfied);
        candidates.push(ConditionCandidate {
            identity: prepared.identity.clone(),
            observation,
            numeric_slots,
            is_numeric_match,
        });
    }
    candidates
}

fn evaluate_numeric_slots(
    constraints: &[NumericConstraint],
    values: &[Option<Decimal>],
) -> Vec<NumericSlotEvidence> {
    constraints
        .iter()
        .enumerate()
        .map(|(slot_index, constraint)| {
            let actual_value = values.get(slot_index).copied().flatten();
            NumericSlotEvidence {
                slot_index,
                actual_value,
                constraint: constraint.clone(),
                is_satisfied: numeric_constraint_satisfied(constraint, actual_value),
            }
        })
        .collect()
}

fn numeric_constraint_satisfied(constraint: &NumericConstraint, actual: Option<Decimal>) -> bool {
    match constraint.mode {
        NumericConstraintMode::Ignore => true,
        NumericConstraintMode::RangeInclusive => matches!(
            (actual, constraint.minimum, constraint.maximum),
            (Some(actual), Some(minimum), Some(maximum)) if actual >= minimum && actual <= maximum
        ),
        NumericConstraintMode::AtLeast => matches!(
            (actual, constraint.minimum),
            (Some(actual), Some(minimum)) if actual >= minimum
        ),
        NumericConstraintMode::AtMost => matches!(
            (actual, constraint.maximum),
            (Some(actual), Some(maximum)) if actual <= maximum
        ),
        NumericConstraintMode::Exactly => matches!(
            (actual, constraint.expected),
            (Some(actual), Some(expected)) if actual == expected
        ),
    }
}

fn build_observation_components(
    candidates: &[Vec<ConditionCandidate>],
) -> HashMap<ObservationIdentity, usize> {
    let mut unique = HashMap::<ObservationIdentity, ModifierObservation>::new();
    for candidate in candidates.iter().flatten() {
        unique
            .entry(candidate.identity.clone())
            .or_insert_with(|| candidate.observation.clone());
    }
    let mut observations: Vec<_> = unique.into_iter().collect();
    observations.sort_by(|(left, _), (right, _)| {
        left.start_line_index
            .cmp(&right.start_line_index)
            .then_with(|| left.physical_line_count.cmp(&right.physical_line_count))
    });
    if observations.is_empty() {
        return HashMap::new();
    }

    let mut union_find = UnionFind::new(observations.len());
    let mut component_end = 0;
    let mut previous = None;
    let mut observation_by_band = HashMap::<String, usize>::new();
    for (index, (identity, observation)) in observations.iter().enumerate() {
        if identity.start_line_index < component_end
            && let Some(previous) = previous
        {
            union_find.union(index, previous);
        }
        component_end = component_end.max(identity.start_line_index + identity.physical_line_count);
        previous = Some(index);
        for band_id in &observation.physical_band_ids {
            if let Some(existing) = observation_by_band.get(band_id) {
                union_find.union(index, *existing);
            } else {
                observation_by_band.insert(band_id.clone(), index);
            }
        }
    }

    let mut component_by_root = HashMap::new();
    let mut result = HashMap::new();
    for (index, (identity, _)) in observations.into_iter().enumerate() {
        let root = union_find.find(index);
        let next_component = component_by_root.len();
        let component = *component_by_root.entry(root).or_insert(next_component);
        result.insert(identity, component);
    }
    result
}

fn build_component_candidates(
    candidates: &[Vec<ConditionCandidate>],
    component_by_observation: &HashMap<ObservationIdentity, usize>,
) -> Vec<Vec<ComponentCandidate>> {
    candidates
        .iter()
        .map(|condition_candidates| {
            let mut by_component = HashMap::<usize, ConditionCandidate>::new();
            for candidate in condition_candidates
                .iter()
                .filter(|candidate| candidate.is_numeric_match)
            {
                let component_id = component_by_observation[&candidate.identity];
                let replace = by_component.get(&component_id).is_none_or(|existing| {
                    candidate.observation.physical_line_count
                        < existing.observation.physical_line_count
                        || (candidate.observation.physical_line_count
                            == existing.observation.physical_line_count
                            && candidate.observation.start_line_index
                                < existing.observation.start_line_index)
                });
                if replace {
                    by_component.insert(component_id, candidate.clone());
                }
            }
            let mut result: Vec<_> = by_component
                .into_iter()
                .map(|(component_id, candidate)| ComponentCandidate {
                    component_id,
                    candidate,
                })
                .collect();
            result.sort_by_key(|edge| edge.component_id);
            result
        })
        .collect()
}

fn find_maximum_component_assignment(
    candidates: &[Vec<ComponentCandidate>],
    condition_order: &[usize],
) -> Vec<Option<ConditionCandidate>> {
    let mut component_assignments = HashMap::new();
    let mut assigned = vec![None; candidates.len()];
    for &condition_index in condition_order {
        let mut visited = HashSet::new();
        try_assign_component(
            condition_index,
            candidates,
            &mut component_assignments,
            &mut assigned,
            &mut visited,
        );
    }
    assigned
}

fn try_assign_component(
    condition_index: usize,
    candidates: &[Vec<ComponentCandidate>],
    component_assignments: &mut HashMap<usize, usize>,
    assigned: &mut [Option<ConditionCandidate>],
    visited: &mut HashSet<usize>,
) -> bool {
    if !visited.insert(condition_index) {
        return false;
    }
    for edge in &candidates[condition_index] {
        let previous = component_assignments.get(&edge.component_id).copied();
        if previous.is_none()
            || try_assign_component(
                previous.unwrap(),
                candidates,
                component_assignments,
                assigned,
                visited,
            )
        {
            component_assignments.insert(edge.component_id, condition_index);
            assigned[condition_index] = Some(edge.candidate.clone());
            return true;
        }
    }
    false
}

fn best_text_candidate(candidates: &[ConditionCandidate]) -> Option<&ConditionCandidate> {
    candidates.iter().max_by(|left, right| {
        left.is_numeric_match
            .cmp(&right.is_numeric_match)
            .then_with(|| {
                left.numeric_slots
                    .iter()
                    .filter(|slot| slot.is_satisfied)
                    .count()
                    .cmp(
                        &right
                            .numeric_slots
                            .iter()
                            .filter(|slot| slot.is_satisfied)
                            .count(),
                    )
            })
            .then(Ordering::Equal)
    })
}

struct UnionFind {
    parents: Vec<usize>,
}

impl UnionFind {
    fn new(count: usize) -> Self {
        Self {
            parents: (0..count).collect(),
        }
    }

    fn find(&mut self, mut item: usize) -> usize {
        while self.parents[item] != item {
            self.parents[item] = self.parents[self.parents[item]];
            item = self.parents[item];
        }
        item
    }

    fn union(&mut self, left: usize, right: usize) {
        let left = self.find(left);
        let right = self.find(right);
        if left != right {
            self.parents[right] = left;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(name: &str, template: &str, minimum: f64) -> AffixCondition {
        AffixCondition::new(name, template, vec![NumericConstraint::at_least(minimum)])
    }

    fn compile(mode: ResultGroupMode, threshold: usize) -> CompiledRuleSet {
        CompiledRuleSet::compile(RuleSetDefinition {
            schema_version: 1,
            name: "contract".into(),
            groups: vec![AcceptableResultGroup {
                name: "result".into(),
                mode,
                required_count: threshold,
                conditions: vec![
                    condition("physical", "#% increased Physical Damage", 170.0),
                    condition("speed", "#% increased Attack Speed", 25.0),
                ],
            }],
        })
        .unwrap()
    }

    #[test]
    fn group_modes_and_numeric_boundaries_match() {
        let any = compile(ResultGroupMode::Any, 1);
        assert!(
            any.evaluate(&["170% increased Physical Damage".into()])
                .is_match
        );
        let all = compile(ResultGroupMode::All, 1);
        assert!(
            !all.evaluate(&["170% increased Physical Damage".into()])
                .is_match
        );
        assert!(
            all.evaluate(&[
                "170% increased Physical Damage".into(),
                "25% increased Attack Speed".into(),
            ])
            .is_match
        );
        let at_least = compile(ResultGroupMode::AtLeast, 2);
        assert!(
            !at_least
                .evaluate(&[
                    "169% increased Physical Damage".into(),
                    "25% increased Attack Speed".into(),
                ])
                .is_match
        );
    }

    #[test]
    fn one_physical_modifier_cannot_satisfy_two_conditions() {
        let rules = CompiledRuleSet::compile(RuleSetDefinition {
            schema_version: 1,
            name: "identity".into(),
            groups: vec![AcceptableResultGroup {
                name: "two views".into(),
                mode: ResultGroupMode::AtLeast,
                required_count: 2,
                conditions: vec![
                    AffixCondition::new(
                        "primary",
                        "#% increased Physical Damage",
                        vec![NumericConstraint::exactly(78.0)],
                    ),
                    AffixCondition::new(
                        "assisted",
                        "#% increased Physical Damage",
                        vec![NumericConstraint::exactly(78.0)],
                    ),
                ],
            }],
        })
        .unwrap();
        let lines = vec!["78% increased Physical Damage".into()];
        let assisted = vec![AssistedModifierObservation {
            physical_band_id: "hybrid-band".into(),
            original_text: "78% increased Physical Damage".into(),
            canonical_target: "#% increased Physical Damage".into(),
            related_physical_band_ids: vec![],
        }];
        assert!(
            rules
                .evaluate_with_identity(&lines, &assisted, &[])
                .is_match
        );
        assert!(
            !rules
                .evaluate_with_identity(
                    &lines,
                    &assisted,
                    &[PhysicalLineIdentity {
                        line_index: 0,
                        physical_band_id: "hybrid-band".into(),
                    }],
                )
                .is_match
        );
    }

    #[test]
    fn traditional_decimal_numeric_constraint_matches() {
        let rules = CompiledRuleSet::compile(RuleSetDefinition {
            schema_version: 1,
            name: "zh".into(),
            groups: vec![AcceptableResultGroup {
                name: "critical".into(),
                mode: ResultGroupMode::All,
                required_count: 1,
                conditions: vec![AffixCondition::new(
                    "暴擊率",
                    "+(3.11—3.8)%暴擊率",
                    vec![NumericConstraint::at_least(3.1)],
                )],
            }],
        })
        .unwrap();
        assert!(rules.evaluate(&["+ 3 · 73 % 暴 擊 率".into()]).is_match);
    }
}
