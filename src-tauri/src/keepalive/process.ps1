$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$request = $env:ZSW_KEEPALIVE_REQUEST | ConvertFrom-Json
$expectedPath = [IO.Path]::GetFullPath([string]$request.executable)
$sessionId = (Get-Process -Id $PID).SessionId
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value

function Read-Instance {
    $all = @(Get-CimInstance Win32_Process -Filter "Name = 'ZCode.exe'")
    $matching = @($all | Where-Object {
        $_.ExecutablePath -and [IO.Path]::GetFullPath($_.ExecutablePath) -eq $expectedPath -and $_.SessionId -eq $sessionId
    })
    $ids = @($matching | ForEach-Object { $_.ProcessId })
    $roots = @($matching | Where-Object { $_.ParentProcessId -notin $ids -and $_.CommandLine -notmatch '(?:^|\s)--type=' })
    if ($roots.Count -gt 1) { throw 'Multiple ZCode instances match. Keepalive requires one local instance.' }
    if ($roots.Count -eq 0) {
        if ($matching.Count -gt 0) { throw 'ZCode child processes are still running.' }
        return $null
    }
    $root = $roots[0]
    $owner = Invoke-CimMethod -InputObject $root -MethodName GetOwnerSid
    if ($owner.Sid -ne $sid) { throw 'ZCode belongs to a different Windows user.' }
    $created = [DateTimeOffset]$root.CreationDate
    $ports = @()
    if ($request.ports) {
        $ports = @(Get-NetTCPConnection -State Listen -OwningProcess $root.ProcessId -ErrorAction SilentlyContinue |
            Where-Object { $_.LocalAddress -eq '127.0.0.1' } | Select-Object -ExpandProperty LocalPort -Unique)
    }
    return [pscustomobject]@{ pid=[uint32]$root.ProcessId; created_ms=$created.ToUnixTimeMilliseconds(); executable=$root.ExecutablePath; ports=$ports }
}

switch ($request.action) {
    'inspect' {
        $instance = Read-Instance
        if ($null -eq $instance) { 'null' }
        else { ConvertTo-Json -InputObject $instance -Compress -Depth 4 }
    }
    'stop' {
        $instance = Read-Instance
        if (!$instance -or $instance.pid -ne $request.pid -or $instance.created_ms -ne $request.created_ms) {
            throw 'ZCode process identity changed; restart cancelled.'
        }
        # Retain the exact live process handle across the PID-based tree operation.
        $process = Get-Process -Id $instance.pid
        $null = $process.Handle
        if ($process.HasExited) { throw 'ZCode exited before restart.' }
        if (([DateTimeOffset]$process.StartTime).ToUnixTimeMilliseconds() -ne $request.created_ms -or [IO.Path]::GetFullPath($process.Path) -ne $expectedPath) {
            throw 'ZCode process identity changed before acquiring its handle.'
        }
        $null = & "$env:SystemRoot\System32\taskkill.exe" /PID $instance.pid /T /F 2>&1
        if ($LASTEXITCODE -ne 0 -and !$process.HasExited) { throw 'Could not stop the selected ZCode instance.' }
        $process.WaitForExit(10000) | Out-Null
        if (!$process.HasExited) { throw 'ZCode is still running.' }
        '{"stopped":true}'
    }
    'start' {
        if (Read-Instance) { throw 'ZCode is already running; launch cancelled.' }
        $arguments = @('--remote-debugging-address=127.0.0.1', '--remote-debugging-port=0')
        $process = Start-Process -FilePath $expectedPath -ArgumentList $arguments -WorkingDirectory ([IO.Path]::GetDirectoryName($expectedPath)) -WindowStyle Hidden -PassThru
        [pscustomobject]@{ pid=$process.Id; created_ms=([DateTimeOffset]$process.StartTime).ToUnixTimeMilliseconds(); executable=$expectedPath; ports=@() } | ConvertTo-Json -Compress
    }
    default { throw 'Unknown keepalive process operation.' }
}
