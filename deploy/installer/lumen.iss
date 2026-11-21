; Lumen installer: the host or the client.
; Build: deploy\build-installer.ps1.
;
; Install modes: the setup starts without
; admin rights. The client goes to the user (%LOCALAPPDATA%\Programs\Lumen),
; so that its silent update needs no UAC; the host goes to the machine
; (Program Files): choosing it in the wizard starts the setup again, elevated
; (/ALLUSERS). Silent: the client — /VERYSILENT /TYPE=client [/RELAUNCH=1];
; the host — /VERYSILENT /ALLUSERS /TYPE=host (from the service, as SYSTEM).
;
; The programs go to {app}. The host is the Windows service "Lumen Host"
;: it runs as SYSTEM and keeps the machine's one
; registration in %ProgramData%\Lumen\host; Lumen Host, the window, manages it
; from any Windows account, and turns the host on and off (the service's start
; type: "Start with Windows"). Its tray icon starts at every user's
; sign-in (HKLM Run, --hidden) and shows only while the host is on. The client
; keeps its data in the user's %APPDATA%\Lumen.
;
; Virtual monitors: the Lumen VDD driver package
; goes to {app}\vdd with every host; the driver itself only with the task
; "vdd" (silent: /TASKS=vdd). Installing over without it (Lumen's update
; too) updates an installed older driver, and installs none where there is
; none (owner's decisions 2026-10-29, 2026-11-12); uninstalling Lumen removes it.
;
; Languages (docs: the owner's decision 2026-11-04): English or Russian, asked
; first, English unless chosen (or chosen at the previous install). The choice
; goes to HKLM\SOFTWARE\Lumen\Language: Lumen Host and Lumen Client open in it
; until the user picks another with the globe (desktop::lang::default).

#define AppName "Lumen"

[Setup]
AppId={{22294F4B-89FB-465B-8DD4-DDEE28DFF9CD}
AppName={#AppName}
AppPublisher=Lumen
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Lumen
DefaultGroupName=Lumen
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
; One folder per mode, no page for it: the host's {autopf} is Program Files,
; the client's the user's Programs.
DisableDirPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutDir}
OutputBaseFilename=lumen-setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=force
; Restart Manager would start the closed apps in the setup's own session —
; session 0 when the service runs the setup. The service starts the tray.
RestartApplications=no
; The language dialog always, English first whatever Windows speaks.
ShowLanguageDialog=yes
LanguageDetectionMethod=none
; The app icon (tools\make-icons.ps1): the setup's own, and in Apps & features
; (a file of its own: a client has no lumen-host-app.exe).
SetupIconFile=..\..\icons\lumen.ico
; The picture at the top right of the pages: the app's icon, as big as the
; uninstaller shows it (the setup showed Inno's own box; tools\make-icons.ps1).
WizardSmallImageFile=..\..\icons\wizard-small.png
UninstallDisplayIcon={app}\lumen.ico

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
en.TypeHost=Host — the computer others connect to (Lumen Host)
ru.TypeHost=Хост — компьютер, к которому подключаются (Lumen Host)
en.TypeClient=Client — the computer you connect from (Lumen Client)
ru.TypeClient=Клиент — компьютер, с которого подключаются (Lumen Client)
en.TaskDesktopIcon=Desktop shortcut
ru.TaskDesktopIcon=Ярлык на рабочем столе
en.TaskVdd=Virtual monitors (Lumen VDD driver): up to two extra monitors, set up in Lumen Host
ru.TaskVdd=Виртуальные мониторы (драйвер Lumen VDD): до двух дополнительных мониторов, настраиваются в Lumen Host
en.StatusWebView2=Installing WebView2…
ru.StatusWebView2=Установка WebView2…
en.StatusFirewall=Firewall rule…
ru.StatusFirewall=Правило брандмауэра…
en.StatusService=The Lumen Host service…
ru.StatusService=Служба Lumen Host…
en.RunHost=Start Lumen Host
ru.RunHost=Запустить Lumen Host
en.RunClient=Start Lumen Client
ru.RunClient=Запустить Lumen Client
en.StatusVdd=The virtual monitor driver (Lumen VDD)…
ru.StatusVdd=Драйвер виртуальных мониторов (Lumen VDD)…
en.VddFailed=The virtual monitor driver was not installed (code %1). Lumen Host works without it; to try again, run this installer once more.
ru.VddFailed=Драйвер виртуальных мониторов не установлен (код %1). Lumen Host работает без них; установку можно повторить, запустив этот установщик снова.
en.ElevateFailed=The setup could not start itself with administrator rights (code %1), which the host needs. Run it from an administrator command prompt: lumen-setup.exe /ALLUSERS
ru.ElevateFailed=Установщик не смог перезапуститься с правами администратора (код %1), а они нужны хосту. Запустите его из командной строки администратора: lumen-setup.exe /ALLUSERS
en.VddLog=Log: %1
ru.VddLog=Журнал: %1
en.PurgeTitle=User data
ru.PurgeTitle=Данные пользователя
en.PurgeSubtitle=Keep the keys and settings for a later install, or delete them.
ru.PurgeSubtitle=Оставить ключи и настройки для следующей установки или удалить их.
en.PurgeGo=&Uninstall
ru.PurgeGo=&Удалить
en.PurgeBox=Delete user data
ru.PurgeBox=Удалить данные пользователя
en.PurgeLead=Select whether to delete them, then click Uninstall.
ru.PurgeLead=Выберите, удалять ли их, и нажмите «Удалить».
en.PurgeHost=The machine's key, registration and token, the settings of the host and its virtual monitors, logs — for all Windows users of this computer. A new install starts from scratch: the machine gets a new token. Its old entry on the server is removed by the administrator in the admin panel.
ru.PurgeHost=Ключ, регистрация и токен машины, настройки хоста и виртуальных мониторов, журналы — у всех пользователей Windows этого компьютера. Новая установка начнётся с нуля: у машины будет новый токен. Старую запись машины на сервере удалит администратор в админке.
en.PurgeClient=This Windows user's client key, name, server, machines and their tokens, settings and logs. A new install starts from scratch: the server sees a new computer, and access must be approved again.
ru.PurgeClient=Ключ клиента этого пользователя Windows, имя, сервер, машины и их токены, настройки и журналы. Новая установка начнётся с нуля: сервер увидит новый компьютер, доступ нужно будет одобрить заново.

[Types]
Name: "host"; Description: "{cm:TypeHost}"
Name: "client"; Description: "{cm:TypeClient}"

[Components]
Name: "host"; Description: "Lumen Host"; Types: host
Name: "client"; Description: "Lumen Client"; Types: client

[Tasks]
Name: "desktopicon"; Description: "{cm:TaskDesktopIcon}"
Name: "vdd"; Description: "{cm:TaskVdd}"; Components: host; Flags: unchecked

[Files]
Source: "{#BinDir}\lumen-host-app.exe"; DestDir: "{app}"; Components: host; Flags: ignoreversion
Source: "{#BinDir}\lumen-host.exe"; DestDir: "{app}"; Components: host; Flags: ignoreversion
Source: "{#BinDir}\lumen-client-app.exe"; DestDir: "{app}"; Components: client; Flags: ignoreversion
Source: "{#BinDir}\lumen-client.exe"; DestDir: "{app}"; Components: client; Flags: ignoreversion
; The C runtime next to the programs (app-local; the rest of the CRT is part of
; Windows 10+). Not linked statically: Defender's ML heuristic flags lumen-host
; built with a static CRT as Trojan:Win32/Bearfoos.A!ml.
; Lumen VDD: the package (signed; lumen-vdd-setup.exe puts its certificate in
; LocalMachine) with every host, so that uninstalling finds it; the setup
; last: it installs the driver when the task is chosen.
Source: "{#VddDir}\lumen_vdd.inf"; DestDir: "{app}\vdd"; Components: host; Flags: ignoreversion
Source: "{#VddDir}\lumen_vdd.cat"; DestDir: "{app}\vdd"; Components: host; Flags: ignoreversion
Source: "{#VddDir}\lumen_vdd.dll"; DestDir: "{app}\vdd"; Components: host; Flags: ignoreversion
Source: "{#VddDir}\lumen-vdd.cer"; DestDir: "{app}\vdd"; Components: host; Flags: ignoreversion
Source: "{#VddDir}\lumen-vdd-setup.exe"; DestDir: "{app}\vdd"; Components: host; Flags: ignoreversion; AfterInstall: InstallVdd
Source: "{#CrtDir}\vcruntime140.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\icons\lumen.ico"; DestDir: "{app}"; Flags: ignoreversion
; The WebView2 runtime the windows need (in Windows 11; installed if missing).
Source: "{#BinDir}\MicrosoftEdgeWebview2Setup.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: NeedsWebView2

[Icons]
; The icon from lumen.ico, not from the exe: Windows caches a shortcut's icon by
; its file, and after an update over an older version the exe's path is the
; same, so its shortcuts kept showing the old icon.
Name: "{group}\Lumen Host"; Filename: "{app}\lumen-host-app.exe"; IconFilename: "{app}\lumen.ico"; Components: host
Name: "{autodesktop}\Lumen Host"; Filename: "{app}\lumen-host-app.exe"; IconFilename: "{app}\lumen.ico"; Components: host; Tasks: desktopicon
Name: "{group}\Lumen Client"; Filename: "{app}\lumen-client-app.exe"; IconFilename: "{app}\lumen.ico"; Components: client
Name: "{autodesktop}\Lumen Client"; Filename: "{app}\lumen-client-app.exe"; IconFilename: "{app}\lumen.ico"; Components: client; Tasks: desktopicon
Name: "{group}\{cm:UninstallProgram,Lumen}"; Filename: "{uninstallexe}"

[Run]
Filename: "{tmp}\MicrosoftEdgeWebview2Setup.exe"; Parameters: "/silent /install"; StatusMsg: "{cm:StatusWebView2}"; Check: NeedsWebView2; Flags: waituntilterminated
; Clients reach the host on UDP 9000 (up to 9009 when several Windows users
; stream): for every network profile — WireGuard and Tailscale adapters are
; often "public".
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Lumen Host (UDP)"""; Components: host; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""Lumen Host (UDP)"" dir=in action=allow protocol=UDP localport=9000-9009 profile=any"; Components: host; StatusMsg: "{cm:StatusFirewall}"; Flags: runhidden
; The tray icon's per-user autostart of earlier versions (now HKLM, above).
Filename: "{sys}\reg.exe"; Parameters: "delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v ""Lumen Host"" /f"; Components: host; Flags: runhidden runasoriginaluser
; The service: installed (or pointed at this program; its start type kept) and started.
Filename: "{app}\lumen-host.exe"; Parameters: "service install"; Components: host; StatusMsg: "{cm:StatusService}"; Flags: runhidden waituntilterminated
Filename: "{app}\lumen-host-app.exe"; Description: "{cm:RunHost}"; Components: host; Flags: postinstall nowait runasoriginaluser skipifsilent
Filename: "{app}\lumen-client-app.exe"; Description: "{cm:RunClient}"; Components: client; Flags: postinstall nowait runasoriginaluser skipifsilent
; A silent update the client started (/RELAUNCH=1): the client again.
Filename: "{app}\lumen-client-app.exe"; Components: client; Flags: nowait; Check: Relaunch

[Registry]
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Lumen Host"; ValueData: """{app}\lumen-host-app.exe"" --hidden"; Components: host; Flags: uninsdeletevalue
; The setup's language: the apps' first one (desktop::lang::default).
Root: HKA; Subkey: "Software\Lumen"; ValueType: string; ValueName: "Language"; ValueData: "{language}"; Flags: uninsdeletevalue uninsdeletekeyifempty

[UninstallRun]
Filename: "{app}\lumen-host.exe"; Parameters: "service uninstall"; Components: host; Flags: runhidden waituntilterminated; RunOnceId: "Service"
; After the service: it held the virtual monitors. Nothing installed: nothing done.
Filename: "{app}\vdd\lumen-vdd-setup.exe"; Parameters: "uninstall"; Components: host; Flags: runhidden waituntilterminated; RunOnceId: "Vdd"
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im lumen-host-app.exe /im lumen-host.exe /im lumen-client-app.exe /im lumen-client.exe"; Flags: runhidden; RunOnceId: "StopApps"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Lumen Host (UDP)"""; Flags: runhidden; RunOnceId: "Firewall"
; The per-user autostart entry of earlier versions (HKCU of the user who set it).
Filename: "{sys}\reg.exe"; Parameters: "delete HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v ""Lumen Host"" /f"; Flags: runhidden; RunOnceId: "Autostart"

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Code]
var
  VddReboot: Boolean;
  { Uninstalling: the user data goes too (the box, or /PURGE=1 when silent). }
  Purge: Boolean;
  PurgeHost: Boolean;
  { The setup started itself elevated for the host: this one just closes. }
  Relaunched: Boolean;

{ The driver, before the service starts ([Run]): the setup's exit code says
  whether Windows needs a restart (3010). A failure leaves Lumen working as
  before, without virtual monitors. Without the task (an update, silent too):
  `update` — the driver only where it is installed and older (the owner's
  decision 2026-11-12), nothing elsewhere. }
procedure InstallVdd();
var
  Code: Integer;
  SetupLog: String;
  Command: String;
  Output: AnsiString;
begin
  if WizardIsTaskSelected('vdd') then
    Command := 'install'
  else
    Command := 'update';
  WizardForm.StatusLabel.Caption := CustomMessage('StatusVdd');
  { What the setup prints (the step that failed and why) goes to vdd\setup.log. }
  SetupLog := ExpandConstant('{app}\vdd\setup.log');
  if not Exec(ExpandConstant('{cmd}'), '/c ""' + ExpandConstant('{app}\vdd\lumen-vdd-setup.exe') + '" ' + Command + ' > "' +
      SetupLog + '" 2>&1"', ExpandConstant('{app}\vdd'), SW_HIDE, ewWaitUntilTerminated, Code) then
    Code := -1;
  if not LoadStringFromFile(SetupLog, Output) then
    Output := '';
  Log(Format('lumen-vdd-setup %s: %d', [Command, Code]));
  Log(String(Output));
  if Code = 3010 then
    VddReboot := True
  else if Code <> 0 then
    SuppressibleMsgBox(FmtMessage(CustomMessage('VddFailed'), [IntToStr(Code)]) + #13#10#13#10 + Trim(String(Output)) + #13#10 +
      FmtMessage(CustomMessage('VddLog'), [SetupLog]), mbError, MB_OK, IDOK);
end;

function NeedRestart(): Boolean;
begin
  Result := VddReboot;
end;

{ An update replaces lumen-host.exe, which the running service holds: stop it
  first (the [Run] entry starts it again). }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  if WizardIsComponentSelected('host') then
  begin
    Exec(ExpandConstant('{sys}\net.exe'), 'stop LumenHost', '', SW_HIDE, ewWaitUntilTerminated, Code);
    { The tray in every session: Restart Manager asks the apps to close, and
      from session 0 it does not reach them all. The service starts it again. }
    Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im lumen-host-app.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  end;
  Result := '';
end;

function Relaunch(): Boolean;
begin
  Result := ExpandConstant('{param:RELAUNCH|0}') = '1';
end;

{ ShellExecuteW itself, not the script's ShellExec: that one passes no owner
  window and asks the shell for no UI, so the UAC prompt (and SmartScreen's,
  for a setup copied from another machine) did not show and Next did nothing.
  The wizard as the owner brings the prompt to the front. }
function ShellExecuteW(hwnd: HWND; lpOperation, lpFile, lpParameters, lpDirectory: String; nShowCmd: Integer): Integer;
  external 'ShellExecuteW@shell32.dll stdcall';

{ The host chosen without admin rights: the same setup again, elevated, for
  all users, with the type and language already chosen. UAC declined: stay. }
function NextButtonClick(CurPageID: Integer): Boolean;
var
  Code: Integer;
begin
  Result := True;
  if (CurPageID = wpSelectComponents) and WizardIsComponentSelected('host') and not IsAdminInstallMode() then
  begin
    Code := ShellExecuteW(WizardForm.Handle, 'runas', ExpandConstant('{srcexe}'),
      '/ALLUSERS /TYPE=host /ELEVATED=1 /LANG=' + ActiveLanguage(), '', SW_SHOW);
    Log(Format('Elevated setup for the host: %d', [Code]));
    if Code > 32 then
    begin
      Relaunched := True;
      WizardForm.Close;
    end
    { 5: UAC declined, the wizard stays as it was. }
    else if Code <> 5 then
      MsgBox(FmtMessage(CustomMessage('ElevateFailed'), [IntToStr(Code)]), mbError, MB_OK);
    Result := False;
  end;
end;

procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  if Relaunched then
    Confirm := False;
end;

{ The elevated setup: the type is already chosen. }
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = wpSelectComponents) and (ExpandConstant('{param:ELEVATED|0}') = '1');
end;

{ Ctrl+Alt+Del from a client: the service calls SendSAS, which Windows
  honours only if the policy SoftwareSASGeneration lets services do it (bit 1;
  bit 2 is for Ease of Access apps, kept as it was). }
procedure CurStepChanged(CurStep: TSetupStep);
var
  V: Cardinal;
begin
  if (CurStep = ssPostInstall) and WizardIsComponentSelected('host') then
  begin
    if not RegQueryDWordValue(HKLM, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System', 'SoftwareSASGeneration', V) then
      V := 0;
    RegWriteDWordValue(HKLM, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System', 'SoftwareSASGeneration', V or 1);
  end;
end;

{ The runtime of one EdgeUpdate key: its version in the registry and its
  msedgewebview2.exe on disk. The registry alone is not enough: Windows Sandbox
  (2026-11-09) has the key, but no runtime — Lumen Host and Lumen Client could
  not start. }
function WebView2At(Root: Integer; Key, Folder: String): Boolean;
var
  V, Dir: String;
begin
  Result := False;
  if not RegQueryStringValue(Root, Key, 'pv', V) or (V = '') or (V = '0.0.0.0') then
    Exit;
  if not RegQueryStringValue(Root, Key, 'location', Dir) or (Dir = '') then
    Dir := Folder;
  Result := FileExists(AddBackslash(Dir) + V + '\msedgewebview2.exe');
  Log(Format('WebView2 %s in %s: %d', [V, Dir, Ord(Result)]));
end;

{ Evergreen WebView2 runtime, per machine or per user. }
function NeedsWebView2(): Boolean;
begin
  Result := not (
    WebView2At(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
      ExpandConstant('{commonpf32}\Microsoft\EdgeWebView\Application')) or
    WebView2At(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
      ExpandConstant('{localappdata}\Microsoft\EdgeWebView\Application')));
end;

{ ---- Uninstalling: "Delete user data" (owner's decision 2026-11-08) ----
  Off by default. The host: %ProgramData%\Lumen (key, registration, monitors,
  update state, logs) and, in every profile, what Lumen Host keeps there
  (host-app.json, its logs, its WebView2 folder). The client: this user's
  %APPDATA%\Lumen (key, settings, trusted servers, window places),
  %LOCALAPPDATA%\Lumen (logs, updates) and its WebView2 folder. Files of the
  other kind stay: a computer may have both a host and a client. }

{ The question as a page of the uninstaller's own window, like the setup's
  pages (the owner's wish 2026-11-08: no separate window). The window shows
  this page first, modal, with "Uninstall" and "Cancel"; then the usual
  progress page. Cancel or the cross: nothing is uninstalled. }
procedure InitializeUninstallProgressForm();
var
  Page: TNewNotebookPage;
  List: TNewCheckListBox;
  Text: TNewStaticText;
  Go: TNewButton;
  Form: TUninstallProgressForm;
  OldName, OldDescription: String;
  CancelEnabled: Boolean;
  CancelResult: Integer;
  Margin: Integer;
begin
  PurgeHost := FileExists(ExpandConstant('{app}\lumen-host.exe'));
  if UninstallSilent() then
  begin
    Purge := ExpandConstant('{param:PURGE|0}') = '1';
    Exit;
  end;
  Form := UninstallProgressForm;
  Page := TNewNotebookPage.Create(Form);
  Page.Notebook := Form.InnerNotebook;
  Page.Parent := Form.InnerNotebook;
  Page.Align := alClient;
  { As the setup's "Select Additional Tasks" page: the text, then the list
    without a border. The uninstaller's page area reaches the window's edges
    (the setup's starts under the subtitle): the same margins by hand. }
  Margin := Form.PageDescriptionLabel.Left - Form.InnerNotebook.Left;
  Text := TNewStaticText.Create(Form);
  Text.Parent := Page;
  Text.Left := Margin;
  Text.Top := 0;
  Text.Width := Form.InnerNotebook.Width - 2 * Margin;
  Text.WordWrap := True;
  if PurgeHost then
    Text.Caption := CustomMessage('PurgeHost') + #13#10#13#10 + CustomMessage('PurgeLead')
  else
    Text.Caption := CustomMessage('PurgeClient') + #13#10#13#10 + CustomMessage('PurgeLead');
  List := TNewCheckListBox.Create(Form);
  List.Parent := Page;
  List.Left := Margin;
  List.Top := Text.Top + Text.Height + ScaleY(8);
  List.Width := Form.InnerNotebook.Width - 2 * Margin;
  List.Height := Form.InnerNotebook.Height - List.Top;
  List.BorderStyle := bsNone;
  List.ParentColor := True;
  List.AddCheckBox(CustomMessage('PurgeBox'), '', 0, False, True, False, True, nil);
  Form.InnerNotebook.ActivePage := Page;
  OldName := Form.PageNameLabel.Caption;
  OldDescription := Form.PageDescriptionLabel.Caption;
  Form.PageNameLabel.Caption := CustomMessage('PurgeTitle');
  Form.PageDescriptionLabel.Caption := CustomMessage('PurgeSubtitle');
  Go := TNewButton.Create(Form);
  Go.Parent := Form;
  Go.Width := Form.CancelButton.Width;
  Go.Height := Form.CancelButton.Height;
  Go.Top := Form.CancelButton.Top;
  Go.Left := Form.CancelButton.Left - Go.Width - ScaleX(10);
  Go.Caption := CustomMessage('PurgeGo');
  Go.ModalResult := mrOk;
  Go.Default := True;
  CancelEnabled := Form.CancelButton.Enabled;
  CancelResult := Form.CancelButton.ModalResult;
  Form.CancelButton.Enabled := True;
  Form.CancelButton.ModalResult := mrCancel;
  Form.ActiveControl := Go;
  if Form.ShowModal() <> mrOk then
    Abort;
  Purge := List.Checked[0];
  Form.CancelButton.Enabled := CancelEnabled;
  Form.CancelButton.ModalResult := CancelResult;
  Form.PageNameLabel.Caption := OldName;
  Form.PageDescriptionLabel.Caption := OldDescription;
  Form.InnerNotebook.ActivePage := Form.InstallingPage;
  Go.Visible := False;
end;

{ Every file in Dir whose name starts with Prefix ('' — all) and does not
  start with Keep ('' — none kept); folders go whole. }
procedure DeleteIn(Dir, Prefix, Keep: String);
var
  F: TFindRec;
  Name: String;
begin
  if FindFirst(AddBackslash(Dir) + '*', F) then
  try
    repeat
      Name := F.Name;
      if (Name = '.') or (Name = '..') then
        continue;
      if (Prefix <> '') and (Pos(Lowercase(Prefix), Lowercase(Name)) <> 1) then
        continue;
      if (Keep <> '') and (Pos(Lowercase(Keep), Lowercase(Name)) = 1) then
        continue;
      if F.Attributes and FILE_ATTRIBUTE_DIRECTORY <> 0 then
        DelTree(AddBackslash(Dir) + Name, True, True, True)
      else
        DeleteFile(AddBackslash(Dir) + Name);
    until not FindNext(F);
  finally
    FindClose(F);
  end;
end;

{ One profile's Lumen files: Roaming and Local are its AppData folders. }
procedure PurgeProfile(Roaming, Local: String; Host: Boolean);
begin
  Log('Purge: ' + Roaming + ', ' + Local);
  if Host then
  begin
    DeleteFile(Roaming + '\Lumen\host-app.json');
    DeleteIn(Local + '\Lumen\logs', 'host', '');
    DelTree(Local + '\io.github.aimbyleet.lumen.host', True, True, True);
  end
  else
  begin
    { Logs: the folder stays for a host's logs, if any. }
    DeleteIn(Roaming + '\Lumen', '', 'host-app.json');
    DeleteIn(Local + '\Lumen\logs', '', 'host');
    DeleteIn(Local + '\Lumen', '', 'logs');
    DelTree(Local + '\io.github.aimbyleet.lumen.client', True, True, True);
  end;
  RemoveDir(Local + '\Lumen\logs');
  RemoveDir(Local + '\Lumen');
  RemoveDir(Roaming + '\Lumen');
end;

{ The host: every profile in ProfileList (default AppData layout). }
procedure PurgeHostProfiles();
var
  Keys: TArrayOfString;
  I: Integer;
  Dir: String;
begin
  if not RegGetSubkeyNames(HKLM, 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList', Keys) then
    Exit;
  for I := 0 to GetArrayLength(Keys) - 1 do
    if RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\' + Keys[I],
        'ProfileImagePath', Dir) and (Pos('%', Dir) = 0) and DirExists(Dir) then
      PurgeProfile(Dir + '\AppData\Roaming', Dir + '\AppData\Local', True);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { The answer: the page above, or /PURGE=1 when silent. }
  if CurUninstallStep = usUninstall then
    Log(Format('Purge user data: %d, host: %d', [Ord(Purge), Ord(PurgeHost)]));
  { After [UninstallRun]: the service and the apps are gone, nothing holds the files. }
  if (CurUninstallStep = usPostUninstall) and Purge then
    if PurgeHost then
    begin
      DelTree(ExpandConstant('{commonappdata}\Lumen'), True, True, True);
      PurgeHostProfiles();
    end
    else
      PurgeProfile(ExpandConstant('{userappdata}'), ExpandConstant('{localappdata}'), False);
end;
