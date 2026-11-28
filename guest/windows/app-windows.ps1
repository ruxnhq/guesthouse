# Runs in the user's session, as only a program there sees that session's windows. The helper itself is
# the C# in app-windows.cs, which the start script put next to this file; it listens for the host on
# port 8769 and answers only a caller that sends the agent's token first. It ends, with the port
# unheard, if it cannot listen, so the next start can try again.
$source = [IO.File]::ReadAllText('C:\gpupv\app-windows.cs', [Text.Encoding]::UTF8)
$references = 'System.Windows.Forms', 'System.Drawing', 'System.Web.Extensions.dll', 'System.Xml'
Add-Type -ReferencedAssemblies $references -TypeDefinition $source
[Guesthouse.AppWindows]::Run((Get-Content C:\gpupv\token.txt -Raw).Trim())
