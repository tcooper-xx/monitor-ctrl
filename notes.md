# Notes — Future Work

## Option A: Custom KMDF Keyboard Filter Driver

Goal: intercept keystrokes below Synergy's kernel-mode blocking so MonitorCtrl
hotkeys fire even when Synergy has captured the keyboard.

---

### Why a kernel driver is necessary

All user-mode approaches have been exhausted:

- `WH_KEYBOARD_LL` — Synergy's hook fires first
- `RegisterRawInputDevices` + `WM_INPUT` (RIDEV_INPUTSINK) — blocked before delivery
- `GetAsyncKeyState` polling — blocked below the async key-state update
- UIAccess manifest (`uiAccess="true"`) — did not resolve the issue
- Interception driver (oblitum) — broken on modern Windows

Synergy appears to use a kernel-mode HID filter driver that intercepts input
before any user-mode mechanism can see it.  A co-installed kernel filter with
higher priority in the keyboard device stack is the only remaining option.

---

### Architecture

```
Physical keyboard
      │
  [kbfiltr.sys]        ← our KMDF filter driver (upper filter on keyboard class)
      │                   sees every keystroke before Synergy
  [kbdclass.sys]       ← Windows keyboard class driver
      │
  [Synergy kernel component]
      │
  user-mode hooks / message queue
```

The driver forwards all keystrokes unmodified (never swallows them) and
simultaneously notifies the MonitorCtrl user-mode process via a named pipe or
DeviceIoControl when a registered combo is detected.

---

### Driver implementation

**Starting point:** Microsoft WDK sample `kbfiltr`
- Location in WDK: `%WDKPath%\src\hid\kbfiltr`
- Language: C (KMDF)
- Implements a complete upper-filter keyboard driver; we add combo detection on top

**Key changes to the sample:**

1. **Combo registration** — MonitorCtrl opens a handle to the driver's device
   object and sends registered combos via `DeviceIoControl` (IOCTL).  Store
   combos in non-paged pool inside the driver.

2. **Keystroke inspection** — In the completion routine for IRP_MJ_READ
   (the path keystrokes travel up the stack), inspect each `KEYBOARD_INPUT_DATA`
   record.  Track modifier state (Ctrl/Alt/Shift/Win) and detect rising edges.

3. **Notification** — When a combo fires, signal MonitorCtrl via one of:
   - A named kernel event (`ZwCreateEvent` / `KeSetEvent`) that the app waits on
   - A named pipe written from the driver's DPC
   - A pending IRP (overlapped `DeviceIoControl`) completed by the driver

   Named event + shared memory is the simplest approach for this use case.

**KEYBOARD_INPUT_DATA fields used:**
```c
typedef struct _KEYBOARD_INPUT_DATA {
    USHORT UnitId;
    USHORT MakeCode;   // scan code
    USHORT Flags;      // KEY_MAKE=0, KEY_BREAK=1, KEY_E0=2, KEY_E1=4
    USHORT Reserved;
    ULONG  ExtraInformation;
} KEYBOARD_INPUT_DATA;
```
`MakeCode` + `Flags & KEY_E0` maps to VK codes using the same scan-code table
already in `src/hotkeys.rs::scancode_to_vk()`.

---

### Driver signing for development (test signing mode)

Production deployment requires an EV code-signing certificate.  For personal /
development use, enable test signing:

```powershell
# Run as Administrator, then reboot
bcdedit /set testsigning on
```

Sign the driver with a self-signed certificate (same approach as the UIAccess
attempt, but applied to the .sys file instead of the .exe):

```powershell
$cert = New-SelfSignedCertificate -Subject "CN=MonitorCtrl Driver Dev" `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -KeyUsage DigitalSignature -Type CodeSigningCert

# Import cert into trusted stores (required for driver signing)
$store = New-Object System.Security.Cryptography.X509Certificates.X509Store("Root","LocalMachine")
$store.Open("ReadWrite"); $store.Add($cert); $store.Close()
$store = New-Object System.Security.Cryptography.X509Certificates.X509Store("TrustedPublisher","LocalMachine")
$store.Open("ReadWrite"); $store.Add($cert); $store.Close()

# Sign the .sys file
& signtool sign /fd sha256 /n "MonitorCtrl Driver Dev" kbfiltr.sys
```

Install as an upper filter on the keyboard class:

```powershell
# Add our driver as an upper filter on the keyboard class
$key = "HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4D36E96B-E325-11CE-BFC1-08002BE10318}"
$existing = (Get-ItemProperty $key).UpperFilters
Set-ItemProperty $key UpperFilters ($existing + "kbfiltr")

# Copy the .sys file
Copy-Item kbfiltr.sys "$env:SystemRoot\System32\drivers\"

# Create the service
sc.exe create kbfiltr type= kernel start= demand binPath= "$env:SystemRoot\System32\drivers\kbfiltr.sys"
sc.exe start kbfiltr
```

---

### MonitorCtrl user-mode changes

Replace `src/hotkeys.rs` with a version that:

1. Opens the driver's named device object:
   ```rust
   CreateFileW(r"\\.\KbFilter", GENERIC_READ | GENERIC_WRITE, ...)
   ```

2. Sends registered combos to the driver via `DeviceIoControl` (IOCTL_KBFILTR_REGISTER_COMBO).

3. Waits on a named event (`OpenEventW`) that the driver signals when a combo fires,
   or uses an overlapped IOCTL that completes with the action index.

The `HotkeyManager` public interface (`new`, `register`, `unregister_all`, `drain_hits`)
stays identical — no changes to `main.rs` or `settings_ui.rs`.

---

### Estimated effort

| Task | Effort |
|---|---|
| KMDF filter driver (based on kbfiltr sample) | ~1–2 days |
| IOCTL interface + combo detection in driver | ~1 day |
| User-mode Rust side (DeviceIoControl FFI) | ~0.5 day |
| Test signing, install scripts, testing | ~0.5 day |
| **Total** | **~3–4 days** |

### References

- WDK kbfiltr sample: https://github.com/microsoft/Windows-driver-samples/tree/main/input/kbfiltr
- KMDF keyboard filter docs: https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/keyboard-and-mouse-class-drivers
- Driver signing for development: https://learn.microsoft.com/en-us/windows-hardware/drivers/install/installing-an-unsigned-driver-during-development-and-test
