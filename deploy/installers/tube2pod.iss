#ifndef Binary
#error Binary is required
#endif
#ifndef OutputDir
#error OutputDir is required
#endif
[Setup]
AppId={{A56FC2D1-5EBB-48E1-A8B0-6DDB59F8B171}
AppName=Tube2Pod
AppVersion=0.1.0
DefaultDirName={localappdata}\Programs\Tube2Pod
DefaultGroupName=Tube2Pod
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=tube2pod-x86_64-pc-windows-msvc
Compression=lzma2
SolidCompression=yes
[Files]
Source: "{#Binary}"; DestDir: "{app}"; Flags: ignoreversion
[Icons]
Name: "{group}\Tube2Pod"; Filename: "{app}\tube2pod.exe"; Parameters: "--desktop"; WorkingDir: "{app}"
[Run]
Filename: "{app}\tube2pod.exe"; Parameters: "--desktop"; Description: "Open Tube2Pod"; Flags: postinstall nowait skipifsilent
