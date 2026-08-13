# Paddle target-support contract (implemented)

This document records the compatibility contract used to close the remaining Traditional-Chinese
corpus cases without accepting raw greedy text. It is now implemented by `poe-alarm-ocr-paddle`:
target characters are scored while model logits remain local to the session, and complete logits
are never returned or retained.

Original design boundary (kept as a parity reference):

```rust
pub struct PaddleTargetSpec {
    pub key: String,
    pub source_template: String,
    pub allow_latin_substitutions: bool,
}

pub struct PaddleMismatchSupport {
    pub expected: String,
    pub actual: String,
    pub greedy_time_step: usize,
    pub evidence_time_step: usize,
    pub rank: usize,
    pub probability: f32,
    pub top_probability: f32,
    pub probability_ratio: f32,
}

pub struct PaddleTargetSupport {
    pub shape_compatible: bool,
    pub strongly_supported: bool,
    pub mismatches: Vec<PaddleMismatchSupport>,
    pub reason: String,
}

pub struct TargetedCtcRecognition {
    pub recognition: CtcRecognition,
    pub supports: Vec<(String, PaddleTargetSupport)>,
}
```

Add one worker command/method accepting `OwnedImage` plus `Vec<PaddleTargetSpec>`. It must execute
one inference, greedy-decode once, calculate every support from the shared output tensor, and send
only the compact result above. The recognition adapter will accept an assisted target only when
`strongly_supported` is true and retain the crop's physical band id.

Parity reference in `.NET` 1.0: `src/PoeAlarm.App/Recognition/PaddleCtcSession.cs`.

- lines 650-663: expected and greedy canonical token counts must be identical;
- lines 665-705: token kinds must match and substitutions are position preserving;
- lines 710-721: canonical equality succeeds immediately; otherwise lexical emission count must
  equal the expected lexical character count;
- lines 724-754: each expected character is looked up in the dictionary and scored; at most two
  mismatches are accepted, each with rank <= 3, probability >= 0.01, and probability ratio >=
  0.012;
- lines 757-785: search the expected class only in the inclusive `center - 2 .. center + 2` time
  window and keep its highest-probability step; rank excludes CTC blank;
- lines 787-813: normalize emitted lexical characters with compatibility normalization and
  lowercase; Chinese substitutions must be one Han character to one Han character. Latin support
  is opt-in, requires words of length >= 4, equal lengths, and at most two changed characters.
