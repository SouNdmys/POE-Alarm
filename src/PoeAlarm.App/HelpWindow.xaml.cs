using System.Diagnostics;
using System.Windows;
using System.Windows.Navigation;
using PoeAlarm.App.Localization;

namespace PoeAlarm.App;

public partial class HelpWindow : Window
{
    public HelpWindow()
    {
        InitializeComponent();
        ApplyText(UiText.Current);
    }

    private void ApplyText(UiStrings text)
    {
        Title = text.HelpTitle;
        TitleText.Text = text.HelpTitle;
        IntroductionText.Text = text.HelpIntroduction;
        Step1TitleText.Text = text.HelpStep1Title;
        Step1BodyText.Text = text.HelpStep1Body;
        Step2TitleText.Text = text.HelpStep2Title;
        Step2BodyText.Text = text.HelpStep2Body;
        Step3TitleText.Text = text.HelpStep3Title;
        Step3BodyText.Text = text.HelpStep3Body;
        TipText.Text = text.HelpTip;
        OcrInstallTitleText.Text = text.HelpOcrTitle;
        OcrInstallBodyText.Text = text.HelpOcrBody;
        OcrInstallCommandTextBox.Text = text.HelpOcrInstallCommand;
        OcrVerifyCommandTextBox.Text = text.HelpOcrVerifyCommand;
        OcrInstallSuccessText.Text = text.HelpOcrSuccess;
        AuthorLabelRun.Text = text.HelpAuthorLabel;
        ContactLabelRun.Text = text.HelpContactLabel;
        SupportTitleText.Text = text.HelpSupportTitle;
        SupportBodyText.Text = text.HelpSupportBody;
        CloseButton.Content = text.Close;
    }

    private void OnContactNavigate(object sender, RequestNavigateEventArgs e)
    {
        try
        {
            Process.Start(new ProcessStartInfo(e.Uri.AbsoluteUri) { UseShellExecute = true });
        }
        catch (Exception exception) when (
            exception is InvalidOperationException or System.ComponentModel.Win32Exception)
        {
            Clipboard.SetText("soundmys1994@gmail.com");
        }

        e.Handled = true;
    }

    private void OnCloseClick(object sender, RoutedEventArgs e) => Close();
}
