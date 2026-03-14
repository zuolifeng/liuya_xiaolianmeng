import argparse
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parent


def _read_text(p: Path) -> str:
    return p.read_text(encoding="utf-8")


def _write_text(p: Path, s: str) -> None:
    p.write_text(s, encoding="utf-8", newline="\n")


def _replace_regex(label: str, s: str, pattern: str, repl: str, flags: int = 0) -> str:
    r = re.compile(pattern, flags)
    if not r.search(s):
        raise SystemExit(f"{label}: pattern not found: {pattern}")
    return r.sub(repl, s, count=0)


def apply_config(cfg: dict, check: bool) -> None:
    server = cfg["server"]
    key = cfg["key"]
    app_name = cfg["app_name"]
    package_name = cfg["package_name"]
    org_name = cfg["org_name"]
    copyright_text = cfg["copyright"]

    hbb_cfg = ROOT / "libs" / "hbb_common" / "src" / "config.rs"
    s = _read_text(hbb_cfg)
    s = _replace_regex(
        "hbb_common config.rs ORG",
        s,
        r'pub static ref ORG: RwLock<String> = RwLock::new\(".*?"\.to_owned\(\)\);',
        f'pub static ref ORG: RwLock<String> = RwLock::new("{org_name}".to_owned());',
    )
    s = _replace_regex(
        "hbb_common config.rs PROD_RENDEZVOUS_SERVER",
        s,
        r'pub static ref PROD_RENDEZVOUS_SERVER: RwLock<String> =[\s\S]*?RwLock::new\(".*?"\.to_owned\(\)\);',
        f'pub static ref PROD_RENDEZVOUS_SERVER: RwLock<String> =\n        RwLock::new("{server}".to_owned());',
        flags=re.MULTILINE,
    )
    s = _replace_regex(
        "hbb_common config.rs APP_NAME",
        s,
        r'pub static ref APP_NAME: RwLock<String> = RwLock::new\(".*?"\.to_owned\(\)\);',
        f'pub static ref APP_NAME: RwLock<String> = RwLock::new("{app_name}".to_owned());',
    )
    s = _replace_regex(
        "hbb_common config.rs RENDEZVOUS_SERVERS",
        s,
        r'pub const RENDEZVOUS_SERVERS: &\[\&str\] = &\[".*?"\];',
        f'pub const RENDEZVOUS_SERVERS: &[&str] = &["{server}"];',
    )
    s = _replace_regex(
        "hbb_common config.rs RS_PUB_KEY",
        s,
        r'pub const RS_PUB_KEY: &str = ".*?";',
        f'pub const RS_PUB_KEY: &str = "{key}";',
    )
    if not check:
        _write_text(hbb_cfg, s)

    cargo_toml = ROOT / "Cargo.toml"
    s = _read_text(cargo_toml)
    exe_name = package_name.split(".")[-1] + ".exe"
    s = _replace_regex(
        "Cargo.toml winres",
        s,
        r'(?ms)^\[package\.metadata\.winres\]\n.*?\n\n',
        "[package.metadata.winres]\n"
        f'LegalCopyright = "{copyright_text}"\n'
        f'ProductName = "{app_name}"\n'
        f'FileDescription = "{app_name}"\n'
        f'OriginalFilename = "{exe_name}"\n\n',
    )
    s = _replace_regex(
        "Cargo.toml bundle name",
        s,
        r'(?m)^name = ".*"$',
        f'name = "{app_name}"',
    )
    s = _replace_regex(
        "Cargo.toml bundle identifier",
        s,
        r'(?m)^identifier = ".*"$',
        f'identifier = "{package_name}"',
    )
    if not check:
        _write_text(cargo_toml, s)

    gradle = ROOT / "flutter" / "android" / "app" / "build.gradle"
    s = _read_text(gradle)
    s = _replace_regex(
        "Android build.gradle applicationId",
        s,
        r'(?m)^\s*applicationId\s+"[^"]+"\s*$',
        f'        applicationId "{package_name}"',
    )
    if not check:
        _write_text(gradle, s)

    for manifest in [
        ROOT / "flutter" / "android" / "app" / "src" / "main" / "AndroidManifest.xml",
        ROOT / "flutter" / "android" / "app" / "src" / "debug" / "AndroidManifest.xml",
        ROOT / "flutter" / "android" / "app" / "src" / "profile" / "AndroidManifest.xml",
    ]:
        s = _read_text(manifest)
        s = _replace_regex(
            f"AndroidManifest package {manifest.name}",
            s,
            r'(?m)^\s*package="[^"]+"\s*$',
            f'    package="{package_name}">',
        )
        if manifest.name == "AndroidManifest.xml" and manifest.parent.name == "main":
            s = _replace_regex(
                "AndroidManifest label",
                s,
                r'(?m)^\s*android:label="[^"]+"\s*$',
                f'        android:label="{app_name}"',
            )
            s = _replace_regex(
                "AndroidManifest DEBUG_BOOT_COMPLETED action",
                s,
                r'(?m)^\s*<action android:name="[^"]+DEBUG_BOOT_COMPLETED" />\s*$',
                f'                <action android:name="{package_name}.DEBUG_BOOT_COMPLETED" />',
            )
        if not check:
            _write_text(manifest, s)

    kt_root = ROOT / "flutter" / "android" / "app" / "src" / "main" / "kotlin"
    for p in kt_root.rglob("*.kt"):
        s = _read_text(p)
        if s.lstrip().startswith("package hbb"):
            continue
        s2 = re.sub(r"(?m)^package\s+com\.[^\s]+\s*$", f"package {package_name}", s)
        s2 = s2.replace(
            'const val DEBUG_BOOT_COMPLETED = "com.carriez.flutter_hbb.DEBUG_BOOT_COMPLETED"',
            f'const val DEBUG_BOOT_COMPLETED = "{package_name}.DEBUG_BOOT_COMPLETED"',
        )
        s2 = s2.replace(
            'const val DEBUG_BOOT_COMPLETED = "com.liuyaxiang.remoteapp.DEBUG_BOOT_COMPLETED"',
            f'const val DEBUG_BOOT_COMPLETED = "{package_name}.DEBUG_BOOT_COMPLETED"',
        )
        s2 = s2.replace(
            "import com.carriez.flutter_hbb.RdClipboardManager",
            f"import {package_name}.RdClipboardManager",
        )
        s2 = s2.replace(
            "import com.liuyaxiang.remoteapp.RdClipboardManager",
            f"import {package_name}.RdClipboardManager",
        )
        if s2 != s and not check:
            _write_text(p, s2)

    ios_info = ROOT / "flutter" / "ios" / "Runner" / "Info.plist"
    s = _read_text(ios_info)
    s = _replace_regex(
        "iOS Info.plist display name",
        s,
        r"(?s)(<key>CFBundleDisplayName</key>\s*<string>).*?(</string>)",
        rf"\1{app_name}\2",
    )
    s = _replace_regex(
        "iOS Info.plist bundle name",
        s,
        r"(?s)(<key>CFBundleName</key>\s*<string>).*?(</string>)",
        rf"\1{app_name}\2",
    )
    s = _replace_regex(
        "iOS Info.plist URL name",
        s,
        r"(?s)(<key>CFBundleURLName</key>\s*<string>).*?(</string>)",
        rf"\1{package_name}\2",
    )
    if not check:
        _write_text(ios_info, s)

    pbxproj = ROOT / "flutter" / "ios" / "Runner.xcodeproj" / "project.pbxproj"
    s = _read_text(pbxproj)
    s = _replace_regex(
        "iOS pbxproj PRODUCT_BUNDLE_IDENTIFIER",
        s,
        r"(?m)^\s*PRODUCT_BUNDLE_IDENTIFIER = .*?;\s*$",
        f"\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = {package_name};",
    )
    if not check:
        _write_text(pbxproj, s)

    export_opts = ROOT / "flutter" / "ios" / "exportOptions.plist"
    if export_opts.exists():
        s = _read_text(export_opts)
        s = _replace_regex(
            "iOS exportOptions provisioningProfiles key",
            s,
            r"(?s)(<key>provisioningProfiles</key>\s*<dict>\s*<key>).*?(</key>)",
            rf"\1{package_name}\2",
        )
        if not check:
            _write_text(export_opts, s)

    google_plist = ROOT / "flutter" / "ios" / "Runner" / "GoogleService-Info.plist"
    if google_plist.exists():
        s = _read_text(google_plist)
        s = _replace_regex(
            "iOS GoogleService BUNDLE_ID",
            s,
            r"(?s)(<key>BUNDLE_ID</key>\s*<string>).*?(</string>)",
            rf"\1{package_name}\2",
        )
        if not check:
            _write_text(google_plist, s)

    mac_xcconfig = ROOT / "flutter" / "macos" / "Runner" / "Configs" / "AppInfo.xcconfig"
    s = _read_text(mac_xcconfig)
    s = _replace_regex(
        "macOS PRODUCT_NAME",
        s,
        r'(?m)^PRODUCT_NAME = .*$',
        f"PRODUCT_NAME = {app_name}",
    )
    s = _replace_regex(
        "macOS PRODUCT_BUNDLE_IDENTIFIER",
        s,
        r'(?m)^PRODUCT_BUNDLE_IDENTIFIER = .*$',
        f"PRODUCT_BUNDLE_IDENTIFIER = {package_name}",
    )
    s = _replace_regex(
        "macOS PRODUCT_COPYRIGHT",
        s,
        r"(?m)^PRODUCT_COPYRIGHT = .*$",
        f"PRODUCT_COPYRIGHT = {copyright_text}",
    )
    if not check:
        _write_text(mac_xcconfig, s)

    for plist in [
        ROOT / "src" / "platform" / "privileges_scripts" / "daemon.plist",
        ROOT / "src" / "platform" / "privileges_scripts" / "agent.plist",
    ]:
        s = _read_text(plist)
        s = _replace_regex(
            f"{plist.name} AssociatedBundleIdentifiers",
            s,
            r"(?s)(<key>AssociatedBundleIdentifiers</key>\s*<string>).*?(</string>)",
            rf"\1{package_name}\2",
        )
        if not check:
            _write_text(plist, s)

    for scpt in [
        ROOT / "src" / "platform" / "privileges_scripts" / "install.scpt",
        ROOT / "src" / "platform" / "privileges_scripts" / "uninstall.scpt",
        ROOT / "src" / "platform" / "privileges_scripts" / "update.scpt",
    ]:
        if not scpt.exists():
            continue
        s = _read_text(scpt)
        s = s.replace("RustDesk", app_name)
        if not check:
            _write_text(scpt, s)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", default=str(ROOT / "private_config.json"))
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    cfg_path = Path(args.config)
    cfg = json.loads(_read_text(cfg_path))
    required = ["server", "key", "app_name", "package_name", "org_name", "copyright"]
    missing = [k for k in required if k not in cfg or not str(cfg[k]).strip()]
    if missing:
        raise SystemExit(f"missing keys in {cfg_path}: {', '.join(missing)}")

    apply_config(cfg, check=args.check)


if __name__ == "__main__":
    main()
