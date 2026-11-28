# Sent by the host through the agent at every start, with the agent this app has in $agent, in Base64,
# and where the guest keeps it in $path. A guest set up by an earlier version has its file replaced,
# and runs the new one from its next boot: the agent that is running now goes on as it is. Setup
# copies the same file from the disc.
$new = [Convert]::FromBase64String($agent)
$old = if (Test-Path -LiteralPath $path) { [IO.File]::ReadAllBytes($path) } else { [byte[]]@() }
if ([Convert]::ToBase64String($old) -eq [Convert]::ToBase64String($new)) {
    'agent current'
} else {
    [IO.File]::WriteAllBytes("$path.new", $new)
    Move-Item -LiteralPath "$path.new" -Destination $path -Force
    'agent updated'
}
