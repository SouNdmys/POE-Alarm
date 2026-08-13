using System.Text;
using PoeAlarm.Core.Matching;

namespace PoeAlarm.Core.Rules;

/// <summary>A validated rule set whose templates are compiled once, outside the frame hot path.</summary>
public sealed class CompiledRuleSet
{
    private readonly CompiledGroup[] _groups;
    private readonly int _maximumPhysicalLineSpan;

    internal CompiledRuleSet(RuleSetDefinition definition, int maximumPhysicalLineSpan)
    {
        ArgumentNullException.ThrowIfNull(definition);
        if (maximumPhysicalLineSpan is < 1 or > FullLineAffixMatcher.MaximumSupportedPhysicalLineSpan)
        {
            throw new ArgumentOutOfRangeException(
                nameof(maximumPhysicalLineSpan),
                $"The line span must be between 1 and " +
                $"{FullLineAffixMatcher.MaximumSupportedPhysicalLineSpan}.");
        }

        if (definition.SchemaVersion != RuleSetDefinition.CurrentSchemaVersion)
        {
            throw new ArgumentException(
                $"Unsupported rule schema version {definition.SchemaVersion}.",
                nameof(definition));
        }

        if (definition.Groups is null)
        {
            throw new ArgumentException(
                "Groups (acceptable results) cannot be null.");
        }

        if (definition.Groups.Count is < 1 or > RuleSetDefinition.MaximumGroups)
        {
            throw new ArgumentException(
                $"A rule set must contain 1-{RuleSetDefinition.MaximumGroups} acceptable results.",
                nameof(definition));
        }

        for (var groupIndex = 0; groupIndex < definition.Groups.Count; groupIndex++)
        {
            var group = definition.Groups[groupIndex];
            if (group is null)
            {
                throw new ArgumentException(
                    $"Acceptable result {groupIndex + 1} cannot be null.");
            }

            if (group.Conditions is null)
            {
                throw new ArgumentException(
                    $"Acceptable result {groupIndex + 1}: Conditions cannot be null.");
            }

            for (var conditionIndex = 0;
                 conditionIndex < group.Conditions.Count;
                 conditionIndex++)
            {
                var condition = group.Conditions[conditionIndex];
                if (condition is null)
                {
                    throw new ArgumentException(
                        $"Acceptable result {groupIndex + 1}, condition " +
                        $"{conditionIndex + 1} cannot be null.");
                }

                if (condition.NumericConstraints is null)
                {
                    throw new ArgumentException(
                        $"Acceptable result {groupIndex + 1}, condition " +
                        $"{conditionIndex + 1}: NumericConstraints " +
                        "(numeric constraints) cannot be null.");
                }

                for (var slot = 0; slot < condition.NumericConstraints.Count; slot++)
                {
                    if (condition.NumericConstraints[slot] is null)
                    {
                        throw new ArgumentException(
                            $"Acceptable result {groupIndex + 1}, condition " +
                            $"{conditionIndex + 1}: numeric constraint {slot + 1} " +
                            "cannot be null.");
                    }
                }
            }
        }

        var totalConditions = definition.Groups.Sum(static group => group.Conditions.Count);
        if (totalConditions > RuleSetDefinition.MaximumConditions)
        {
            throw new ArgumentException(
                $"A rule set may contain at most {RuleSetDefinition.MaximumConditions} conditions.",
                nameof(definition));
        }

        ValidateStableNames(definition);
        _maximumPhysicalLineSpan = maximumPhysicalLineSpan;
        _groups = definition.Groups
            .Select((group, index) => CompileGroup(group, index, maximumPhysicalLineSpan))
            .ToArray();
        Definition = definition;
        Targets = _groups
            .SelectMany(static group => group.Conditions)
            .Select(static condition => condition.Matcher)
            .GroupBy(static matcher =>
                $"{matcher.MaximumLineSpan}:{matcher.Template.Text}",
                StringComparer.Ordinal)
            .Select(static group => group.First())
            .ToArray();
    }

    public RuleSetDefinition Definition { get; }

    /// <summary>Deduplicated lexical targets for one-pass, target-aware OCR adapters.</summary>
    public IReadOnlyList<FullLineAffixMatcher> Targets { get; }

    public RuleEvaluationResult Evaluate(IReadOnlyList<string> physicalLines)
    {
        ArgumentNullException.ThrowIfNull(physicalLines);

        var observations = PrepareObservations(physicalLines, _maximumPhysicalLineSpan);
        var evaluations = new ResultGroupEvaluation[_groups.Length];
        int? firstMatchedGroup = null;
        for (var index = 0; index < _groups.Length; index++)
        {
            evaluations[index] = EvaluateGroup(_groups[index], index, observations);
            if (firstMatchedGroup is null && evaluations[index].IsMatch)
            {
                firstMatchedGroup = index;
            }
        }

        return new RuleEvaluationResult(firstMatchedGroup is not null, firstMatchedGroup, evaluations);
    }

    private static void ValidateStableNames(RuleSetDefinition definition)
    {
        var duplicateGroup = definition.Groups
            .Select(static group => group.Name?.Trim())
            .Where(static name => !string.IsNullOrWhiteSpace(name))
            .GroupBy(static name => name!, StringComparer.Ordinal)
            .FirstOrDefault(static group => group.Count() > 1);
        if (duplicateGroup is not null)
        {
            throw new ArgumentException(
                $"Acceptable result name '{duplicateGroup.Key}' is duplicated.");
        }

        for (var groupIndex = 0; groupIndex < definition.Groups.Count; groupIndex++)
        {
            var duplicateCondition = definition.Groups[groupIndex].Conditions
                .Select(static condition => condition.Name?.Trim())
                .Where(static name => !string.IsNullOrWhiteSpace(name))
                .GroupBy(static name => name!, StringComparer.Ordinal)
                .FirstOrDefault(static group => group.Count() > 1);
            if (duplicateCondition is not null)
            {
                throw new ArgumentException(
                    $"Acceptable result {groupIndex + 1} contains duplicate condition name " +
                    $"'{duplicateCondition.Key}'.");
            }
        }
    }

    private static CompiledGroup CompileGroup(
        AcceptableResultGroup group,
        int groupIndex,
        int maximumPhysicalLineSpan)
    {
        ArgumentNullException.ThrowIfNull(group);
        if (group.Conditions.Count == 0)
        {
            throw new ArgumentException($"Acceptable result {groupIndex + 1} has no conditions.");
        }

        var requiredCount = group.Mode switch
        {
            ResultGroupMode.Any => 1,
            ResultGroupMode.All => group.Conditions.Count,
            ResultGroupMode.AtLeast when group.RequiredCount >= 1 &&
                                             group.RequiredCount <= group.Conditions.Count =>
                group.RequiredCount,
            ResultGroupMode.AtLeast => throw new ArgumentException(
                $"Acceptable result {groupIndex + 1} requires between 1 and " +
                $"{group.Conditions.Count} conditions."),
            _ => throw new ArgumentException(
                $"Acceptable result {groupIndex + 1} has an unsupported mode."),
        };

        var conditions = group.Conditions
            .Select((condition, index) =>
                CompileCondition(condition, groupIndex, index, maximumPhysicalLineSpan))
            .ToArray();
        return new CompiledGroup(group, requiredCount, conditions);
    }

    private static CompiledCondition CompileCondition(
        AffixCondition condition,
        int groupIndex,
        int conditionIndex,
        int maximumPhysicalLineSpan)
    {
        ArgumentNullException.ThrowIfNull(condition);
        FullLineAffixMatcher matcher;
        try
        {
            matcher = new FullLineAffixMatcher(condition.Template, maximumPhysicalLineSpan);
        }
        catch (Exception exception) when (exception is ArgumentException)
        {
            throw new ArgumentException(
                $"Acceptable result {groupIndex + 1}, condition {conditionIndex + 1}: " +
                exception.Message,
                exception);
        }

        var numericSlotCount = matcher.Template.Tokens.Count(static token =>
            token.Kind != AffixTokenKind.Word);
        if (condition.NumericConstraints.Count > numericSlotCount)
        {
            throw new ArgumentException(
                $"Acceptable result {groupIndex + 1}, condition {conditionIndex + 1} defines " +
                $"{condition.NumericConstraints.Count} numeric constraints for a " +
                $"{numericSlotCount}-slot template.");
        }

        var constraints = new NumericConstraint[numericSlotCount];
        for (var slot = 0; slot < numericSlotCount; slot++)
        {
            var constraint = slot < condition.NumericConstraints.Count
                ? condition.NumericConstraints[slot]
                : NumericConstraint.Ignored();
            ValidateConstraint(constraint, groupIndex, conditionIndex, slot);
            constraints[slot] = constraint;
        }

        var constrainedSlots = constraints.Count(static item =>
            item.Mode != NumericConstraintMode.Ignore);
        var constraintSpecificity = constraints.Sum(ConstraintSpecificity);
        var lexicalTokens = matcher.Template.Tokens.Count(static item =>
            item.Kind == AffixTokenKind.Word);
        return new CompiledCondition(
            condition,
            matcher,
            constraints,
            constrainedSlots,
            constraintSpecificity,
            lexicalTokens,
            conditionIndex);
    }

    private static void ValidateConstraint(
        NumericConstraint constraint,
        int groupIndex,
        int conditionIndex,
        int slotIndex)
    {
        var valid = constraint.Mode switch
        {
            NumericConstraintMode.Ignore => true,
            NumericConstraintMode.RangeInclusive =>
                constraint.Minimum is not null &&
                constraint.Maximum is not null &&
                constraint.Minimum <= constraint.Maximum,
            NumericConstraintMode.AtLeast => constraint.Minimum is not null,
            NumericConstraintMode.AtMost => constraint.Maximum is not null,
            NumericConstraintMode.Exactly => constraint.Expected is not null,
            _ => false,
        };
        if (!valid)
        {
            throw new ArgumentException(
                $"Acceptable result {groupIndex + 1}, condition {conditionIndex + 1}, " +
                $"numeric slot {slotIndex + 1} has an invalid {constraint.Mode} constraint.");
        }
    }

    private static int ConstraintSpecificity(NumericConstraint constraint) =>
        constraint.Mode switch
        {
            NumericConstraintMode.Exactly => 4,
            NumericConstraintMode.RangeInclusive => 3,
            NumericConstraintMode.AtLeast or NumericConstraintMode.AtMost => 2,
            NumericConstraintMode.Ignore => 0,
            _ => 0,
        };

    private static ResultGroupEvaluation EvaluateGroup(
        CompiledGroup group,
        int groupIndex,
        IReadOnlyList<PreparedObservation> observations)
    {
        var candidates = new List<ConditionCandidate>[group.Conditions.Length];
        for (var conditionIndex = 0; conditionIndex < group.Conditions.Length; conditionIndex++)
        {
            candidates[conditionIndex] = FindCandidates(group.Conditions[conditionIndex], observations);
        }

        var conditionOrder = Enumerable.Range(0, group.Conditions.Length)
            .OrderByDescending(index => group.Conditions[index].ConstrainedSlotCount)
            .ThenByDescending(index => group.Conditions[index].ConstraintSpecificity)
            .ThenByDescending(index => group.Conditions[index].LexicalTokenCount)
            .ThenBy(index => group.Conditions[index].OriginalIndex)
            .ToArray();
        var componentByObservation = BuildObservationComponents(candidates);
        var componentCandidates = BuildComponentCandidates(candidates, componentByObservation);
        var assignedCandidates = FindMaximumComponentAssignment(
            componentCandidates,
            conditionOrder);

        var conditionEvaluations = new ConditionEvaluation[group.Conditions.Length];
        var matchedCount = 0;
        for (var conditionIndex = 0; conditionIndex < group.Conditions.Length; conditionIndex++)
        {
            var compiled = group.Conditions[conditionIndex];
            var assigned = assignedCandidates[conditionIndex];
            var bestTextCandidate = assigned ?? BestTextCandidate(candidates[conditionIndex]);
            if (assigned is not null)
            {
                matchedCount++;
            }

            conditionEvaluations[conditionIndex] = new ConditionEvaluation(
                conditionIndex,
                string.IsNullOrWhiteSpace(compiled.Definition.Name)
                    ? compiled.Definition.Template
                    : compiled.Definition.Name,
                compiled.Definition.Template,
                assigned is not null,
                bestTextCandidate?.Observation,
                bestTextCandidate?.NumericSlots ?? []);
        }

        return new ResultGroupEvaluation(
            groupIndex,
            string.IsNullOrWhiteSpace(group.Definition.Name)
                ? $"Result {groupIndex + 1}"
                : group.Definition.Name,
            group.Definition.Mode,
            group.RequiredCount,
            matchedCount,
            matchedCount >= group.RequiredCount,
            conditionEvaluations);
    }

    private static List<ConditionCandidate> FindCandidates(
        CompiledCondition condition,
        IReadOnlyList<PreparedObservation> observations)
    {
        var matches = new List<ConditionCandidate>();
        foreach (var prepared in observations)
        {
            if (prepared.PhysicalLineCount > condition.Matcher.MaximumLineSpan ||
                !condition.Matcher.IsMatch(prepared.Canonical))
            {
                continue;
            }

            var observation = prepared.ToObservation();
            var numericEvidence = EvaluateNumericSlots(
                condition.Constraints,
                observation.NumericValues);
            matches.Add(new ConditionCandidate(
                prepared.Identity,
                observation,
                numericEvidence,
                numericEvidence.All(static evidence => evidence.IsSatisfied)));
        }

        return matches;
    }

    private static IReadOnlyList<PreparedObservation> PrepareObservations(
        IReadOnlyList<string> physicalLines,
        int maximumPhysicalLineSpan)
    {
        var observations = new List<PreparedObservation>(physicalLines.Count * 2);
        for (var start = 0; start < physicalLines.Count; start++)
        {
            if (string.IsNullOrWhiteSpace(physicalLines[start]))
            {
                continue;
            }

            var combined = new StringBuilder();
            var maximumSpan = Math.Min(maximumPhysicalLineSpan, physicalLines.Count - start);
            for (var span = 1; span <= maximumSpan; span++)
            {
                var line = physicalLines[start + span - 1];
                if (string.IsNullOrWhiteSpace(line))
                {
                    break;
                }

                if (combined.Length > 0)
                {
                    combined.Append(' ');
                }

                combined.Append(line.Trim());
                var original = combined.ToString();
                var canonical = AffixCanonicalizer.Normalize(original);
                observations.Add(new PreparedObservation(start, span, original, canonical));
            }
        }

        return observations;
    }

    private static IReadOnlyList<NumericSlotEvidence> EvaluateNumericSlots(
        IReadOnlyList<NumericConstraint> constraints,
        IReadOnlyList<decimal?> values)
    {
        var evidence = new NumericSlotEvidence[constraints.Count];
        for (var slot = 0; slot < constraints.Count; slot++)
        {
            var actual = slot < values.Count ? values[slot] : null;
            var constraint = constraints[slot];
            evidence[slot] = new NumericSlotEvidence(
                slot,
                actual,
                constraint,
                IsSatisfied(constraint, actual));
        }

        return evidence;
    }

    private static bool IsSatisfied(NumericConstraint constraint, decimal? actual) =>
        constraint.Mode switch
        {
            NumericConstraintMode.Ignore => true,
            NumericConstraintMode.RangeInclusive =>
                actual is not null &&
                actual >= constraint.Minimum &&
                actual <= constraint.Maximum,
            NumericConstraintMode.AtLeast =>
                actual is not null && actual >= constraint.Minimum,
            NumericConstraintMode.AtMost =>
                actual is not null && actual <= constraint.Maximum,
            NumericConstraintMode.Exactly =>
                actual is not null && actual == constraint.Expected,
            _ => false,
        };

    private static IReadOnlyDictionary<ObservationIdentity, int> BuildObservationComponents(
        IReadOnlyList<List<ConditionCandidate>> candidates)
    {
        var observations = candidates
            .SelectMany(static items => items)
            .Select(static candidate => candidate.Identity)
            .Distinct()
            .OrderBy(static identity => identity.StartLineIndex)
            .ThenBy(static identity => identity.PhysicalLineCount)
            .ToArray();
        var result = new Dictionary<ObservationIdentity, int>(observations.Length);
        var component = -1;
        var componentEnd = -1;
        foreach (var observation in observations)
        {
            var end = observation.StartLineIndex + observation.PhysicalLineCount;
            // Physical spans are half-open. Adjacent lines can be two independent
            // modifiers; spans sharing any line are one ambiguous modifier component.
            if (component < 0 || observation.StartLineIndex >= componentEnd)
            {
                component++;
                componentEnd = end;
            }
            else
            {
                componentEnd = Math.Max(componentEnd, end);
            }

            result[observation] = component;
        }

        return result;
    }

    private static List<ComponentCandidate>[] BuildComponentCandidates(
        IReadOnlyList<List<ConditionCandidate>> candidates,
        IReadOnlyDictionary<ObservationIdentity, int> componentByObservation)
    {
        var result = new List<ComponentCandidate>[candidates.Count];
        for (var conditionIndex = 0; conditionIndex < candidates.Count; conditionIndex++)
        {
            result[conditionIndex] = candidates[conditionIndex]
                .Where(static candidate => candidate.IsNumericMatch)
                .GroupBy(candidate => componentByObservation[candidate.Identity])
                .Select(static group => new ComponentCandidate(
                    group.Key,
                    group.OrderBy(static candidate => candidate.Observation.PhysicalLineCount)
                        .ThenBy(static candidate => candidate.Observation.StartLineIndex)
                        .First()))
                .OrderBy(static edge => edge.ComponentId)
                .ToList();
        }

        return result;
    }

    private static ConditionCandidate?[] FindMaximumComponentAssignment(
        IReadOnlyList<List<ComponentCandidate>> candidates,
        IReadOnlyList<int> conditionOrder)
    {
        var componentAssignments = new Dictionary<int, int>();
        var assignedCandidates = new ConditionCandidate?[candidates.Count];
        foreach (var conditionIndex in conditionOrder)
        {
            _ = TryAssignComponent(
                conditionIndex,
                candidates,
                componentAssignments,
                assignedCandidates,
                []);
        }

        return assignedCandidates;
    }

    private static bool TryAssignComponent(
        int conditionIndex,
        IReadOnlyList<List<ComponentCandidate>> candidates,
        IDictionary<int, int> componentAssignments,
        ConditionCandidate?[] assignedCandidates,
        HashSet<int> visitedConditions)
    {
        if (!visitedConditions.Add(conditionIndex))
        {
            return false;
        }

        foreach (var edge in candidates[conditionIndex])
        {
            var wasAssigned = componentAssignments.TryGetValue(
                edge.ComponentId,
                out var previousCondition);
            if (!wasAssigned ||
                TryAssignComponent(
                    previousCondition,
                    candidates,
                    componentAssignments,
                    assignedCandidates,
                    visitedConditions))
            {
                componentAssignments[edge.ComponentId] = conditionIndex;
                assignedCandidates[conditionIndex] = edge.Candidate;
                return true;
            }
        }

        return false;
    }

    private static ConditionCandidate? BestTextCandidate(
        IReadOnlyList<ConditionCandidate> candidates) =>
        candidates
            .OrderByDescending(static candidate => candidate.IsNumericMatch)
            .ThenByDescending(static candidate =>
                candidate.NumericSlots.Count(static slot => slot.IsSatisfied))
            .FirstOrDefault();

    private sealed record CompiledGroup(
        AcceptableResultGroup Definition,
        int RequiredCount,
        CompiledCondition[] Conditions);

    private sealed record CompiledCondition(
        AffixCondition Definition,
        FullLineAffixMatcher Matcher,
        NumericConstraint[] Constraints,
        int ConstrainedSlotCount,
        int ConstraintSpecificity,
        int LexicalTokenCount,
        int OriginalIndex);

    private readonly record struct ObservationIdentity(
        int StartLineIndex,
        int PhysicalLineCount,
        string CanonicalText);

    private sealed class PreparedObservation
    {
        private readonly int _startLineIndex;
        private readonly string _originalText;
        private ModifierObservation? _observation;

        public PreparedObservation(
            int startLineIndex,
            int physicalLineCount,
            string originalText,
            CanonicalAffix canonical)
        {
            _startLineIndex = startLineIndex;
            _originalText = originalText;
            PhysicalLineCount = physicalLineCount;
            Canonical = canonical;
            Identity = new ObservationIdentity(startLineIndex, physicalLineCount, canonical.Text);
        }

        public int PhysicalLineCount { get; }

        public int StartLineIndex => _startLineIndex;

        public CanonicalAffix Canonical { get; }

        public ObservationIdentity Identity { get; }

        public ModifierObservation ToObservation() =>
            _observation ??= new ModifierObservation(
                _startLineIndex,
                PhysicalLineCount,
                _originalText,
                Canonical.Text,
                AffixValueExtractor.Extract(_originalText));
    }

    private sealed record ConditionCandidate(
        ObservationIdentity Identity,
        ModifierObservation Observation,
        IReadOnlyList<NumericSlotEvidence> NumericSlots,
        bool IsNumericMatch);

    private sealed record ComponentCandidate(
        int ComponentId,
        ConditionCandidate Candidate);
}
