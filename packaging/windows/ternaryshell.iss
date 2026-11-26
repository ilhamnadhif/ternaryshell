; Inno Setup script for the Windows installer (TernaryShell-<version>-windows-x64-setup.exe).
;
;   iscc /DAppVersion=1.4.0 /DSourceExe=..\..\target\release\ternaryshell.exe /DOutputDir=..\..\dist packaging\windows\ternaryshell.iss
;
; Installs for the current user by default (no administrator rights needed), or for all users
; when chosen in the first dialog. An update installs over the previous version (same AppId)
; and closes a running TernaryShell first. The in-app updater runs it with /SILENT /update=1, which
; opens TernaryShell again when it is done.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\ternaryshell.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{E18F47EC-5E3F-4881-A972-5F64305785A5}
AppName=TernaryShell
AppVersion={#AppVersion}
AppVerName=TernaryShell {#AppVersion}
AppPublisher=TernaryShell contributors
AppPublisherURL=https://github.com/ilhamnadhif/ternaryshell
AppSupportURL=https://github.com/ilhamnadhif/ternaryshell/issues
AppUpdatesURL=https://github.com/ilhamnadhif/ternaryshell/releases
DefaultDirName={autopf}\TernaryShell
DefaultGroupName=TernaryShell
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
SetupIconFile=ternaryshell.ico
UninstallDisplayIcon={app}\ternaryshell.exe
UninstallDisplayName=TernaryShell
OutputDir={#OutputDir}
OutputBaseFilename=TernaryShell-{#AppVersion}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add ternaryshell to PATH (open connections from a terminal: ternaryshell web-1)"; GroupDescription: "Command line:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "ternaryshell.exe"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\TernaryShell"; Filename: "{app}\ternaryshell.exe"
Name: "{autodesktop}\TernaryShell"; Filename: "{app}\ternaryshell.exe"; Tasks: desktopicon

[Registry]
; ternaryshell.exe on PATH (per user, or for all users in an administrative install).
Root: HKA; Subkey: "{code:EnvKey}"; ValueType: expandsz; ValueName: "Path"; ValueData: "{code:AppendedPath}"; \
  Check: NeedsAddPath(ExpandConstant('{app}')); Tasks: addtopath

[Run]
Filename: "{app}\ternaryshell.exe"; Description: "{cm:LaunchProgram,TernaryShell}"; Flags: nowait postinstall skipifsilent
; After an update from the app: open TernaryShell again, as the user (not elevated).
Filename: "{app}\ternaryshell.exe"; Flags: nowait runasoriginaluser; Check: IsAppUpdate

[Code]
function IsAppUpdate: Boolean;
begin
  Result := ExpandConstant('{param:update|0}') = '1';
end;

function EnvKey(Param: String): String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function EnvRootKey: Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function NormalizePathEntry(Entry: String): String;
begin
  Result := Trim(Entry);
  while (Length(Result) > 3) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

function NeedsAddPath(Dir: String): Boolean;
var
  Paths, NormDir, Entry: String;
  P: Integer;
begin
  NormDir := Uppercase(NormalizePathEntry(Dir));
  if not RegQueryStringValue(EnvRootKey, EnvKey(''), 'Path', Paths) then
  begin
    Result := True;
    exit;
  end;
  while Paths <> '' do
  begin
    P := Pos(';', Paths);
    if P > 0 then
    begin
      Entry := Copy(Paths, 1, P - 1);
      Delete(Paths, 1, P);
    end else
    begin
      Entry := Paths;
      Paths := '';
    end;
    if Uppercase(NormalizePathEntry(Entry)) = NormDir then
    begin
      Result := False;
      exit;
    end;
  end;
  Result := True;
end;

function AppendedPath(Param: String): String;
var
  Paths, Dir: String;
begin
  Dir := ExpandConstant('{app}');
  if not RegQueryStringValue(EnvRootKey, EnvKey(''), 'Path', Paths) or (Trim(Paths) = '') then
  begin
    Result := Dir;
    exit;
  end;
  if Paths[Length(Paths)] = ';' then
    Result := Paths + Dir
  else
    Result := Paths + ';' + Dir;
end;

procedure RemoveFromPath(Dir: String);
var
  Paths, NewPaths, Entry, NormDir: String;
  P: Integer;
  Changed: Boolean;
begin
  if not RegQueryStringValue(EnvRootKey, EnvKey(''), 'Path', Paths) then
    exit;
  NormDir := Uppercase(NormalizePathEntry(Dir));
  NewPaths := '';
  Changed := False;
  while Paths <> '' do
  begin
    P := Pos(';', Paths);
    if P > 0 then
    begin
      Entry := Copy(Paths, 1, P - 1);
      Delete(Paths, 1, P);
    end else
    begin
      Entry := Paths;
      Paths := '';
    end;
    if Uppercase(NormalizePathEntry(Entry)) = NormDir then
      Changed := True
    else if Entry <> '' then
    begin
      if NewPaths <> '' then
        NewPaths := NewPaths + ';';
      NewPaths := NewPaths + Entry;
    end;
  end;
  if Changed then
    RegWriteExpandStringValue(EnvRootKey, EnvKey(''), 'Path', NewPaths);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
