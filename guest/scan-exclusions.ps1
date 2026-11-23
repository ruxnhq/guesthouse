# Sent by the host through the agent, which runs as SYSTEM. The host puts $data, the folders as JSON in
# Base64, and $change, add or remove, first. Each folder goes to Defender's own setting, one at a time,
# and is checked again here: a full path of plain characters, never a pattern. Defender stays on, and
# the folders are left out of real-time scanning only. It says done and how many, or why not.
$folderShape = '\A[A-Za-z]:(\\[A-Za-z0-9 ._()-]+)+\z'
try {
    $folders = @(([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($data)) | ConvertFrom-Json).folders)
    $done = 0
    foreach ($folder in $folders) {
        $path = "$folder"
        if ($path -notmatch $folderShape -or $path -match '\\\.\.?(\\|\z)') { "not a folder: $path"; return }
        if ($change -eq 'add') {
            Add-MpPreference -ExclusionPath $path -ErrorAction Stop
        } elseif ($change -eq 'remove') {
            Remove-MpPreference -ExclusionPath $path -ErrorAction Stop
        } else {
            'nothing to do'
            return
        }
        $done++
    }
    "done $done"
} catch {
    "not done: $($_.Exception.Message)"
}
