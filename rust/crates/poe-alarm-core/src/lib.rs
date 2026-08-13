//! Platform-independent matching and composite-rule engine for POE Alarm.
//!
//! Templates are compiled once. Runtime evaluation accepts OCR physical lines,
//! performs full-affix matching, applies numeric constraints, and enforces that
//! one physical modifier can satisfy at most one condition in a result group.

pub mod decimal;
pub mod matching;
pub mod rules;

pub use decimal::{Decimal, DecimalParseError};
pub use matching::{
    AffixToken, AffixTokenKind, CanonicalAffix, FullLineAffixMatcher, LogicalAffixMatch,
    canonicalize, extract_values, normalize_numeric_presentation,
};
pub use rules::{
    AcceptableResultGroup, AffixCondition, AssistedModifierObservation, CompiledRuleSet,
    ConditionEvaluation, ModifierObservation, NumericConstraint, NumericConstraintMode,
    NumericSlotEvidence, PhysicalLineIdentity, ResultGroupEvaluation, ResultGroupMode,
    RuleEvaluationResult, RuleSetDefinition, RuleValidationError,
};
