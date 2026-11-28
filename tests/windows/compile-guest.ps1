# Compiles the C# that the guest's scripts compile, the way the guest does: Windows PowerShell 5.1's
# Add-Type, which runs the .NET Framework's own C# 5 compiler. Each piece compiles in a PowerShell of its
# own, as in the guest, where no two of them share one. Nothing is run.
#   app-windows.cs      with csc as a library at warning level 4, warnings as errors, and then with
#                       app-windows.ps1's own Add-Type line. csc's messages carry the file's lines.
#   Add-Type's sources  in the other scripts, found in each script's syntax tree.
#   api-check.ps1       by its own -SelfCheck, which compiles every probe and checks the struct sizes.
# CI runs it on Windows. With -ListOnly it says what it would compile and compiles nothing, which works
# under pwsh on any system.
param([switch]$ListOnly)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$guest = Join-Path $repo 'guest\windows'
$inCi = $env:GITHUB_ACTIONS -eq 'true'
$script:problems = 0

# Scripts whose Add-Type source isn't written in them.
$readFromFile = @{ 'app-windows.ps1' = 'app-windows.cs' }
$selfChecked = @('api-check.ps1')

function Say-Problem([string]$file, [int]$line, [string]$message) {
    $script:problems++
    $where = if ($line -gt 0) { "${file}:$line" } else { $file }
    "FAILED $where $message"
    if ($inCi) {
        $flat = $message -replace '%', '%25' -replace "`r", '' -replace "`n", '%0A'
        if ($line -gt 0) { "::error file=$file,line=$line::$flat" } else { "::error file=$file::$flat" }
    }
}

# A program's output and errors as lines. Windows PowerShell 5.1 turns a native program's stderr into
# errors, which would stop this script at the first one under 'Stop'.
function Invoke-Native([string]$program, [string[]]$arguments) {
    $ErrorActionPreference = 'Continue'
    & $program @arguments 2>&1 | ForEach-Object { "$_" }
}

function Start-Group([string]$title) { if ($inCi) { "::group::$title" } else { "== $title" } }
function Stop-Group { if ($inCi) { '::endgroup::' } }

# The value a variable is given in a script, when it is given one that can be read without running it.
function Get-Assigned($ast, [string]$name) {
    $found = @($ast.FindAll({
                param($node)
                $node -is [Management.Automation.Language.AssignmentStatementAst] -and
                $node.Left -is [Management.Automation.Language.VariableExpressionAst] -and
                $node.Left.VariablePath.UserPath -eq $name
            }, $true))
    if ($found.Count -ne 1) { return $null }
    $right = $found[0].Right
    if ($right -isnot [Management.Automation.Language.CommandExpressionAst]) { return $null }
    try { return , $right.Expression.SafeGetValue() } catch { return $null }
}

function Get-Value($ast, $argument) {
    if ($null -eq $argument) { return $null }
    if ($argument -is [Management.Automation.Language.VariableExpressionAst]) {
        return Get-Assigned $ast $argument.VariablePath.UserPath
    }
    try { return , $argument.SafeGetValue() } catch { return $null }
}

# Each Add-Type in a guest script that compiles source: the script, its line, the source, the
# assemblies it names and the line of the script the source starts on.
function Get-GuestSources {
    foreach ($file in Get-ChildItem -LiteralPath $guest -Filter '*.ps1' | Sort-Object Name) {
        $name = "guest/windows/$($file.Name)"
        $errors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$null, [ref]$errors)
        if ($errors) { Say-Problem $name $errors[0].Extent.StartLineNumber "does not parse: $($errors[0].Message)"; continue }
        $calls = $ast.FindAll({
                param($node)
                $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Add-Type'
            }, $true)
        foreach ($call in $calls) {
            $bound = [Management.Automation.Language.StaticParameterBinder]::BindCommand($call, $true).BoundParameters
            if ($bound.ContainsKey('AssemblyName')) { continue }
            $line = $call.Extent.StartLineNumber
            if ($selfChecked -contains $file.Name) { continue }
            $references = @()
            if ($bound.ContainsKey('ReferencedAssemblies')) {
                $named = Get-Value $ast $bound['ReferencedAssemblies'].Value
                $references = if ($null -eq $named) { $null } else { @($named) }
            }
            $source = $null
            $offset = 0
            if ($readFromFile.ContainsKey($file.Name)) {
                $source = [IO.File]::ReadAllText((Join-Path $guest $readFromFile[$file.Name]), [Text.Encoding]::UTF8)
                $name = "guest/windows/$($readFromFile[$file.Name])"
            } elseif ($bound.ContainsKey('TypeDefinition')) {
                $argument = $bound['TypeDefinition'].Value
                if ($argument -is [Management.Automation.Language.VariableExpressionAst]) {
                    $assigned = $ast.Find({
                            param($node)
                            $node -is [Management.Automation.Language.AssignmentStatementAst] -and
                            $node.Left.VariablePath.UserPath -eq $argument.VariablePath.UserPath
                        }, $true)
                    if ($assigned) { $argument = $assigned.Right.Expression }
                }
                if ($argument -is [Management.Automation.Language.StringConstantExpressionAst]) {
                    $source = $argument.Value
                    # A here-string's text starts on the line after its @'.
                    $offset = $argument.Extent.StartLineNumber
                    if ($argument.StringConstantType -eq 'SingleQuoted' -or $argument.StringConstantType -eq 'DoubleQuoted') { $offset-- }
                }
            }
            if ($null -eq $source -or $null -eq $references) {
                Say-Problem "guest/windows/$($file.Name)" $line 'Add-Type compiles something this check cannot find in the script. Teach tests/windows/compile-guest.ps1 where it is.'
                continue
            }
            [pscustomobject]@{ Name = $name; Script = "guest/windows/$($file.Name)"; Line = $line; Source = $source; References = $references; Offset = $offset }
        }
    }
}

$sources = @(Get-GuestSources)
if ($ListOnly) {
    foreach ($item in $sources) {
        "$($item.Script):$($item.Line) compiles $($item.Name) from line $($item.Offset + 1), $($item.Source.Length) characters, references: $($item.References -join ', ')"
    }
    foreach ($name in $selfChecked) { "guest/windows/${name} compiles its own probes with -SelfCheck" }
    if ($script:problems) { exit 1 }
    exit 0
}
if ($PSVersionTable.PSEdition -ne 'Desktop') { throw 'This needs Windows PowerShell 5.1, powershell.exe, which is what the guest runs.' }
$work = Join-Path ([IO.Path]::GetTempPath()) "guesthouse-compile-$PID"
New-Item -ItemType Directory -Force $work | Out-Null

# app-windows.cs with csc, for messages with the file's own lines.
$csc = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) 'csc.exe'
$appWindows = $sources | Where-Object { $_.Name -eq 'guest/windows/app-windows.cs' }
if (-not $appWindows) {
    Say-Problem 'guest/windows/app-windows.ps1' 0 'its Add-Type of app-windows.cs was not found'
} else {
    Start-Group "csc: guest/windows/app-windows.cs (C# 5, /warn:4 /warnaserror+)"
    $arguments = @('/nologo', '/target:library', '/warn:4', '/warnaserror+', "/out:$(Join-Path $work 'app-windows.dll')")
    $arguments += $appWindows.References | ForEach-Object { "/reference:$_" -replace '(?<!\.dll)$', '.dll' }
    $arguments += Join-Path $guest 'app-windows.cs'
    "$csc $($arguments -join ' ')"
    $said = Invoke-Native $csc $arguments
    $code = $LASTEXITCODE
    $said
    Stop-Group
    foreach ($message in $said) {
        if ($message -match '\((\d+),\d+\): (error|warning) (.*)$') { Say-Problem 'guest/windows/app-windows.cs' ([int]$Matches[1]) "$($Matches[2]) $($Matches[3])" }
    }
    if ($code -ne 0 -and -not ($said -match '\(\d+,\d+\): (error|warning)')) { Say-Problem 'guest/windows/app-windows.cs' 0 "csc stopped with exit code $code" }
    if ($code -eq 0) { 'ok   csc guest/windows/app-windows.cs' }
}

# Each source with Add-Type, in a PowerShell of its own.
$child = Join-Path $work 'add-type.ps1'
Set-Content -LiteralPath $child -Encoding UTF8 -Value @'
param([string]$SourceFile, [string]$References)
# Every compiler message is printed, and only then does it fail.
$ErrorActionPreference = 'Continue'
$source = [IO.File]::ReadAllText($SourceFile, [Text.Encoding]::UTF8)
$failed = $false
try {
    & {
        if ($References) {
            Add-Type -ReferencedAssemblies ($References -split '\|') -TypeDefinition $source
        } else {
            Add-Type -TypeDefinition $source
        }
    } 2>&1 | ForEach-Object { $failed = $true; "$_" }
} catch {
    $failed = $true
    "$_"
}
if ($failed) { exit 1 }
'@
$number = 0
foreach ($item in $sources) {
    $number++
    $sourceFile = Join-Path $work "source-$number.cs"
    [IO.File]::WriteAllText($sourceFile, $item.Source, (New-Object Text.UTF8Encoding($false)))
    $label = if ($item.Name -eq $item.Script) { "$($item.Script):$($item.Line)" } else { "$($item.Name) as $($item.Script):$($item.Line) loads it" }
    # Windows PowerShell drops an empty argument on its way to a program, so no -References is given
    # when there are none.
    $arguments = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $child, '-SourceFile', $sourceFile)
    if ($item.References) { $arguments += @('-References', ($item.References -join '|')) }
    $said = Invoke-Native powershell.exe $arguments
    if ($LASTEXITCODE -eq 0) { "ok   Add-Type $label"; continue }
    Start-Group "Add-Type: $label failed"
    $said
    Stop-Group
    # Windows PowerShell's compiler says FILE.cs(LINE) : MESSAGE, and Roslyn (LINE,COLUMN): MESSAGE.
    $lines = @($said | Select-String -Pattern '(?:\.cs\((?<line>\d+)\)\s*:|^\((?<line>\d+),\d+\):)\s*(?<message>.*)$' | ForEach-Object { $_.Matches[0] })
    if (-not $lines) { Say-Problem $item.Script $item.Line "Add-Type failed: $($said -join ' ')" }
    foreach ($found in $lines) {
        $file = if ($item.Name -eq $item.Script) { $item.Script } else { $item.Name }
        Say-Problem $file ([int]$found.Groups['line'].Value + $item.Offset) "Add-Type: $($found.Groups['message'].Value.Trim())"
    }
}

# api-check.ps1's probes, by its own check.
foreach ($name in $selfChecked) {
    $said = Invoke-Native powershell.exe @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $guest $name), '-SelfCheck')
    $code = $LASTEXITCODE
    Start-Group "guest/windows/$name -SelfCheck"
    $said
    Stop-Group
    if ($code -eq 0) { "ok   guest/windows/$name -SelfCheck" } else { Say-Problem "guest/windows/$name" 0 "-SelfCheck found $code problem(s): $(($said | Select-Object -Last 1) -join '')" }
}

Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
if ($script:problems) {
    "$($script:problems) problem(s) in the guest's C#"
    exit 1
}
"the guest's C# compiles: $($sources.Count) source(s), and api-check's probes"
