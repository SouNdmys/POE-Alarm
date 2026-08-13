namespace PoeAlarm.App.RulesUi;

internal sealed class RuleEditorStrings
{
    public static RuleEditorStrings For(bool isEnglish) =>
        isEnglish ? English : SimplifiedChinese;

    public required string WindowTitle { get; init; }

    public required string Heading { get; init; }

    public required string Introduction { get; init; }

    public required string RuleSetNameLabel { get; init; }

    public required string RuleSetNamePlaceholder { get; init; }

    public required string DisplayValueWarningTitle { get; init; }

    public required string DisplayValueWarning { get; init; }

    public required string AcceptableResultsHeading { get; init; }

    public required string OrExplanation { get; init; }

    public required string AddResult { get; init; }

    public required string DeleteResult { get; init; }

    public required string ResultNameLabel { get; init; }

    public required string GroupModeLabel { get; init; }

    public required string RequiredCountLabel { get; init; }

    public required string ConditionsHeading { get; init; }

    public required string AddCondition { get; init; }

    public required string DeleteCondition { get; init; }

    public required string ConditionNameLabel { get; init; }

    public required string TemplateLabel { get; init; }

    public required string TemplateHelp { get; init; }

    public required string NumericModeLabel { get; init; }

    public required string MinimumLabel { get; init; }

    public required string MaximumLabel { get; init; }

    public required string ExactValueLabel { get; init; }

    public required string Cancel { get; init; }

    public required string Save { get; init; }

    public required string ValidationTitle { get; init; }

    public required string ValidationPrefix { get; init; }

    public required string EnterTemplate { get; init; }

    public required string InvalidTemplate { get; init; }

    public required string NoNumericSlots { get; init; }

    public required string NumericSlotsFoundFormat { get; init; }

    public required string SlotLabelFormat { get; init; }

    public required string SlotPlainNumber { get; init; }

    public required string SlotPercent { get; init; }

    public required string SlotNegativeNumber { get; init; }

    public required string SlotNegativePercent { get; init; }

    public required string ResultDefaultNameFormat { get; init; }

    public required string ConditionDefaultNameFormat { get; init; }

    public required string ResultFallbackNameFormat { get; init; }

    public required string GroupSummaryAnyFormat { get; init; }

    public required string GroupSummaryAllFormat { get; init; }

    public required string GroupSummaryAtLeastFormat { get; init; }

    public required string CounterFormat { get; init; }

    public required string GroupLimitReached { get; init; }

    public required string ConditionLimitReached { get; init; }

    public required string CannotDeleteOnlyGroup { get; init; }

    public required string CannotDeleteOnlyCondition { get; init; }

    public required string InvalidNumberFormat { get; init; }

    public required string RangeOrderErrorFormat { get; init; }

    public required string AnyMode { get; init; }

    public required string AllMode { get; init; }

    public required string AtLeastMode { get; init; }

    public required string IgnoreMode { get; init; }

    public required string RangeMode { get; init; }

    public required string AtLeastNumericMode { get; init; }

    public required string AtMostNumericMode { get; init; }

    public required string ExactlyMode { get; init; }

    private static RuleEditorStrings SimplifiedChinese { get; } = new()
    {
        WindowTitle = "多词缀命中规则",
        Heading = "多词缀命中规则",
        Introduction = "每个“可接受结果”都是一种值得停手的组合；命中任意一个结果就会报警。",
        RuleSetNameLabel = "规则集名称",
        RuleSetNamePlaceholder = "例如：暴击珠宝",
        DisplayValueWarningTitle = "显示值说明",
        DisplayValueWarning = "催化剂、品质和特殊效果可能改变装备提示框中的数值。程序只比较屏幕上看到的值，不计算装备的原始数值。",
        AcceptableResultsHeading = "可接受结果",
        OrExplanation = "任意一种结果命中就会报警",
        AddResult = "+ 添加另一种可接受结果",
        DeleteResult = "删除结果",
        ResultNameLabel = "结果名称",
        GroupModeLabel = "需要命中",
        RequiredCountLabel = "至少命中条数",
        ConditionsHeading = "词缀条件",
        AddCondition = "+ 添加词缀条件",
        DeleteCondition = "删除",
        ConditionNameLabel = "条件名称",
        TemplateLabel = "完整词缀模板",
        TemplateHelp = "粘贴完整词缀，不要只填关键词。程序会自动找出其中的数值。",
        NumericModeLabel = "约束方式",
        MinimumLabel = "最小值",
        MaximumLabel = "最大值",
        ExactValueLabel = "目标值",
        Cancel = "取消",
        Save = "验证并保存",
        ValidationTitle = "规则无法保存",
        ValidationPrefix = "请修正规则后再保存：",
        EnterTemplate = "请输入完整词缀模板。",
        InvalidTemplate = "模板无效",
        NoNumericSlots = "没有找到数值；此条件只比较完整文字。",
        NumericSlotsFoundFormat = "找到 {0} 个数值。下方设置按它们在词缀中的顺序对应。",
        SlotLabelFormat = "数值 {0} · {1}",
        SlotPlainNumber = "普通数值",
        SlotPercent = "百分比",
        SlotNegativeNumber = "负数",
        SlotNegativePercent = "负百分比",
        ResultDefaultNameFormat = "可接受结果 {0}",
        ConditionDefaultNameFormat = "词缀 {0}",
        ResultFallbackNameFormat = "可接受结果 {0}",
        GroupSummaryAnyFormat = "任意 1 条 / 共 {0} 条",
        GroupSummaryAllFormat = "全部 {0} 条",
        GroupSummaryAtLeastFormat = "至少 {0} 条 / 共 {1} 条",
        CounterFormat = "{0}/{1} 个结果 · {2}/{3} 条条件",
        GroupLimitReached = "最多只能添加 8 个可接受结果。",
        ConditionLimitReached = "一个规则集最多只能包含 32 条条件。",
        CannotDeleteOnlyGroup = "规则集必须保留至少一个可接受结果。",
        CannotDeleteOnlyCondition = "每个可接受结果必须保留至少一条条件。",
        InvalidNumberFormat = "{0} 的第 {1} 个数值：{2}不是有效数值。",
        RangeOrderErrorFormat = "{0} 的第 {1} 个数值：最小值不能大于最大值。",
        AnyMode = "命中任意 1 条",
        AllMode = "命中全部",
        AtLeastMode = "命中指定条数",
        IgnoreMode = "忽略数值",
        RangeMode = "范围（含边界）",
        AtLeastNumericMode = "至少（≥）",
        AtMostNumericMode = "至多（≤）",
        ExactlyMode = "精确等于",
    };

    private static RuleEditorStrings English { get; } = new()
    {
        WindowTitle = "Multi-affix rules",
        Heading = "Multi-affix rules",
        Introduction = "Each acceptable result is a combination worth stopping for. Matching any result triggers the alarm.",
        RuleSetNameLabel = "Rule-set name",
        RuleSetNamePlaceholder = "e.g. Critical jewel",
        DisplayValueWarningTitle = "Displayed values",
        DisplayValueWarning = "Catalysts, quality, and special effects can change values shown in the item tooltip. The app compares only what appears on screen; it does not calculate the item's original values.",
        AcceptableResultsHeading = "Acceptable results",
        OrExplanation = "Matching any one of these results triggers the alert",
        AddResult = "+ Add another acceptable result",
        DeleteResult = "Delete result",
        ResultNameLabel = "Result name",
        GroupModeLabel = "Matches needed",
        RequiredCountLabel = "Required matches",
        ConditionsHeading = "Affix conditions",
        AddCondition = "+ Add affix condition",
        DeleteCondition = "Delete",
        ConditionNameLabel = "Condition name",
        TemplateLabel = "Complete affix template",
        TemplateHelp = "Paste the complete affix instead of a keyword. Values are found automatically.",
        NumericModeLabel = "Constraint",
        MinimumLabel = "Minimum",
        MaximumLabel = "Maximum",
        ExactValueLabel = "Target value",
        Cancel = "Cancel",
        Save = "Validate and save",
        ValidationTitle = "Rule cannot be saved",
        ValidationPrefix = "Fix the following rule problem before saving:",
        EnterTemplate = "Enter a complete affix template.",
        InvalidTemplate = "Invalid template",
        NoNumericSlots = "No values found; this condition compares only the complete text.",
        NumericSlotsFoundFormat = "Found {0} value(s). The settings below follow their order in the affix.",
        SlotLabelFormat = "Value {0} · {1}",
        SlotPlainNumber = "number",
        SlotPercent = "percentage",
        SlotNegativeNumber = "negative number",
        SlotNegativePercent = "negative percentage",
        ResultDefaultNameFormat = "Acceptable result {0}",
        ConditionDefaultNameFormat = "Affix {0}",
        ResultFallbackNameFormat = "Acceptable result {0}",
        GroupSummaryAnyFormat = "Any 1 of {0}",
        GroupSummaryAllFormat = "All {0}",
        GroupSummaryAtLeastFormat = "At least {0} of {1}",
        CounterFormat = "{0}/{1} results · {2}/{3} conditions",
        GroupLimitReached = "A rule set can contain at most 8 acceptable results.",
        ConditionLimitReached = "A rule set can contain at most 32 conditions.",
        CannotDeleteOnlyGroup = "A rule set must retain at least one acceptable result.",
        CannotDeleteOnlyCondition = "Each acceptable result must retain at least one condition.",
        InvalidNumberFormat = "{0}, value {1}: {2} is not a valid number.",
        RangeOrderErrorFormat = "{0}, value {1}: the minimum cannot exceed the maximum.",
        AnyMode = "Match any one",
        AllMode = "Match all",
        AtLeastMode = "Match a chosen number",
        IgnoreMode = "Ignore value",
        RangeMode = "Inclusive range",
        AtLeastNumericMode = "At least (≥)",
        AtMostNumericMode = "At most (≤)",
        ExactlyMode = "Exactly",
    };
}
