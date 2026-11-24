# Sent by the host through the agent, with $request first: list. In a scope of its own, as the agent
# runs this among its own $client and $writer.
& {
    $helper = [System.Net.Sockets.TcpClient]::new('127.0.0.1', 8769)
    $stream = $helper.GetStream()
    $writer = [System.IO.StreamWriter]::new($stream)
    $writer.WriteLine((Get-Content C:\gpupv\token.txt -Raw).Trim() + ' ' + $request)
    $writer.Flush()
    'answer ' + [System.IO.StreamReader]::new($stream).ReadToEnd()
    $helper.Close()
}
