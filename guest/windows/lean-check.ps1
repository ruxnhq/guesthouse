# Sent by the host through the agent, which runs as SYSTEM, with $data first: lean Windows' list as JSON
# in Base64. It only reads. Each name on the list that Windows provisions again, or that an account has
# again, is said on a line of its own, back and the name, and checked ends it. Names are compared whole.
$nameShape = '\A[A-Za-z0-9][A-Za-z0-9.]{1,127}\z'
try {
    $list = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($data)) | ConvertFrom-Json
    $names = @($list.apps | ForEach-Object { "$_" } | Where-Object { $_ -match $nameShape })
    # A provisioned package's DisplayName is its package name, not words in a language.
    $there = @(Get-AppxProvisionedPackage -Online -ErrorAction Stop | ForEach-Object { $_.DisplayName })
    $there += @(Get-AppxPackage -AllUsers -ErrorAction Stop | ForEach-Object { $_.Name })
    $names | Where-Object { $there -contains $_ } | ForEach-Object { "back $_" }
    'checked'
} catch {
    "not checked: $($_.Exception.Message)"
}
