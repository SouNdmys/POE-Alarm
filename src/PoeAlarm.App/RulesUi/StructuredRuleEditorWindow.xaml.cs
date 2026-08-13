using System.Windows;
using System.Windows.Controls;
using PoeAlarm.Core.Matching;
using PoeAlarm.Core.Rules;

namespace PoeAlarm.App.RulesUi;

/// <summary>
/// Editor for the versioned multi-affix rule set. The supplied definition is copied into
/// editable view models; <see cref="Result"/> is assigned only after successful compilation.
/// </summary>
public partial class StructuredRuleEditorWindow : Window
{
    private readonly RuleEditorViewModel viewModel;

    public StructuredRuleEditorWindow(
        RuleSetDefinition? definition,
        bool isEnglish,
        int maximumPhysicalLineSpan = FullLineAffixMatcher.MaximumPhysicalLineSpan)
    {
        InitializeComponent();
        viewModel = new RuleEditorViewModel(
            definition,
            isEnglish,
            maximumPhysicalLineSpan);
        DataContext = viewModel;
        Title = viewModel.Text.WindowTitle;
    }

    public StructuredRuleEditorWindow(
        Window owner,
        RuleSetDefinition? definition,
        bool isEnglish,
        int maximumPhysicalLineSpan = FullLineAffixMatcher.MaximumPhysicalLineSpan)
        : this(definition, isEnglish, maximumPhysicalLineSpan)
    {
        ArgumentNullException.ThrowIfNull(owner);
        Owner = owner;
    }

    /// <summary>The validated replacement definition, or null when the editor is cancelled.</summary>
    public RuleSetDefinition? Result { get; private set; }

    /// <summary>Alias that makes the successful-dialog contract explicit at call sites.</summary>
    public RuleSetDefinition? SavedDefinition => Result;

    /// <summary>
    /// Opens a modal editor owned by <paramref name="owner"/> and returns a validated replacement.
    /// A null return value means cancel; the supplied definition is never mutated.
    /// </summary>
    public static RuleSetDefinition? Edit(
        Window owner,
        RuleSetDefinition? definition,
        bool isEnglish,
        int maximumPhysicalLineSpan = FullLineAffixMatcher.MaximumPhysicalLineSpan)
    {
        ArgumentNullException.ThrowIfNull(owner);
        var window = new StructuredRuleEditorWindow(
            owner,
            definition,
            isEnglish,
            maximumPhysicalLineSpan);
        return window.ShowDialog() == true ? window.Result : null;
    }

    private void OnAddGroup(object sender, RoutedEventArgs e)
    {
        if (!viewModel.TryAddGroup(out var error))
        {
            ShowInformation(error!);
        }
    }

    private void OnDeleteGroup(object sender, RoutedEventArgs e)
    {
        if (sender is not FrameworkElement { DataContext: ResultGroupEditorViewModel group })
        {
            return;
        }

        e.Handled = true;
        if (!viewModel.TryRemoveGroup(group, out var error))
        {
            ShowInformation(error!);
        }
    }

    private void OnAddCondition(object sender, RoutedEventArgs e)
    {
        if (sender is not FrameworkElement { DataContext: ResultGroupEditorViewModel group })
        {
            return;
        }

        if (!viewModel.TryAddCondition(group, out var error))
        {
            ShowInformation(error!);
        }
    }

    private void OnDeleteCondition(object sender, RoutedEventArgs e)
    {
        if (sender is not FrameworkElement { DataContext: AffixConditionEditorViewModel condition })
        {
            return;
        }

        var group = FindAncestorDataContext<ResultGroupEditorViewModel>(sender as DependencyObject);
        if (group is null)
        {
            return;
        }

        if (!viewModel.TryRemoveCondition(group, condition, out var error))
        {
            ShowInformation(error!);
        }
    }

    private void OnSave(object sender, RoutedEventArgs e)
    {
        if (!viewModel.TryBuildDefinition(out var definition, out var errors))
        {
            MessageBox.Show(
                this,
                $"{viewModel.Text.ValidationPrefix}{Environment.NewLine}{Environment.NewLine}" +
                string.Join(Environment.NewLine, errors.Select(static error => $"• {error}")),
                viewModel.Text.ValidationTitle,
                MessageBoxButton.OK,
                MessageBoxImage.Warning);
            return;
        }

        Result = definition;
        DialogResult = true;
    }

    private void OnCancel(object sender, RoutedEventArgs e)
    {
        Result = null;
        DialogResult = false;
    }

    private void ShowInformation(string message) =>
        MessageBox.Show(
            this,
            message,
            viewModel.Text.WindowTitle,
            MessageBoxButton.OK,
            MessageBoxImage.Information);

    private static T? FindAncestorDataContext<T>(DependencyObject? start)
        where T : class
    {
        for (var current = start; current is not null; current = System.Windows.Media.VisualTreeHelper.GetParent(current))
        {
            if (current is FrameworkElement { DataContext: T value })
            {
                return value;
            }
        }

        return null;
    }
}
