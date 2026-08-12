using PoeAlarm.TransientReplay;

try
{
    var options = ReplayOptions.Parse(args);
    if (options.ShowHelp)
    {
        ReplayOptions.PrintHelp();
        return 0;
    }

    return await ReplayBenchmark.RunAsync(options);
}
catch (ArgumentException exception)
{
    Console.Error.WriteLine($"Argument error: {exception.Message}");
    Console.Error.WriteLine("Run with --help for usage.");
    return 2;
}
catch (Exception exception)
{
    Console.Error.WriteLine($"Transient replay failed: {exception.GetType().Name}: {exception.Message}");
    Console.Error.WriteLine(exception.StackTrace);
    return 1;
}
