"""ComfyUI-Manager compatible custom-node post-install bridge.

The launcher executes this source in-memory with its configured ComfyUI
interpreter.  An installed, enabled ComfyUI-Manager is preferred.  The local
implementation is used only when Manager cannot be loaded before post-install
work starts.
"""

import configparser
import json
import locale
import os
import re
import runpy
import shlex
import shutil
import subprocess
import sys
import traceback


STATUS_PATH = os.path.abspath(sys.argv[1])
COMFY_PATH = os.path.abspath(sys.argv[2])
REPO_PATH = os.path.abspath(sys.argv[3])
CURRENT_STAGE = "bootstrap"


def write_status(state, backend, stage, message=""):
    payload = {
        "state": state,
        "backend": backend,
        "stage": stage,
        "message": str(message),
    }
    temporary = f"{STATUS_PATH}.{os.getpid()}.tmp"
    with open(temporary, "w", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, STATUS_PATH)


def command_stage(command):
    values = [str(value) for value in command]
    if values and os.path.basename(values[-1]).lower() == "install.py":
        return "install_script"
    lowered = [value.lower() for value in values]
    if "install" in lowered and ("pip" in lowered or "uv" in lowered):
        return "requirements"
    return "repair"


def find_manager():
    custom_nodes = os.path.join(COMFY_PATH, "custom_nodes")
    try:
        entries = list(os.scandir(custom_nodes))
    except OSError:
        return None
    entries.sort(key=lambda item: (item.name.lower() != "comfyui-manager", item.name.lower()))
    for entry in entries:
        if not entry.is_dir() or entry.name.lower().endswith(".disabled"):
            continue
        if entry.name.lower() != "comfyui-manager":
            continue
        cli = os.path.join(entry.path, "cm-cli.py")
        core = os.path.join(entry.path, "glob", "manager_core.py")
        if os.path.isfile(cli) and os.path.isfile(core):
            return os.path.abspath(entry.path)
    return None


def try_manager(manager_path):
    """Return None when bootstrap failed, otherwise a process exit code."""
    global CURRENT_STAGE
    cli_path = os.path.join(manager_path, "cm-cli.py")
    previous_argv = sys.argv[:]
    try:
        # `runpy` does not give the target script the normal script-directory
        # import precedence. Put Manager's own paths first so a custom node
        # cannot accidentally shadow modules such as `manager_util`.
        sys.path.insert(0, os.path.join(manager_path, "glob"))
        sys.path.insert(0, manager_path)
        sys.argv = [cli_path]
        scope = runpy.run_path(cli_path, run_name="comfyui_tui_manager_bridge")
        manager_util = scope["manager_util"]
        core = scope["core"]
        unified_manager = scope["unified_manager"]
        if not hasattr(unified_manager, "execute_install_script"):
            raise AttributeError("Manager has no execute_install_script API")
    except BaseException as error:
        print(f"[post-install] ComfyUI-Manager bootstrap failed: {error}", file=sys.stderr)
        traceback.print_exc()
        write_status("bootstrap_failed", "manager", "bootstrap", error)
        return None
    finally:
        sys.argv = previous_argv

    original_run_script = core.manager_funcs.run_script

    def tracked_run_script(command, cwd="."):
        global CURRENT_STAGE
        CURRENT_STAGE = command_stage(command)
        return original_run_script(command, cwd=cwd)

    core.manager_funcs.run_script = tracked_run_script
    write_status("manager_started", "manager", "bootstrap")
    print(f"[post-install] using ComfyUI-Manager at {manager_path}")
    try:
        outer_fixer = manager_util.PIPFixer(
            manager_util.get_installed_packages(), COMFY_PATH, core.manager_files_path
        )
        ok = unified_manager.execute_install_script(
            "", REPO_PATH, instant_execution=True
        )
        if not ok:
            write_status(
                "failed", "manager", CURRENT_STAGE, "Manager reported post-install failure"
            )
            return 1
        CURRENT_STAGE = "repair"
        outer_fixer.fix_broken()
        write_status("completed", "manager", "complete")
        return 0
    except BaseException as error:
        stage = command_stage(getattr(error, "cmd", []))
        if stage == "repair" and CURRENT_STAGE != "bootstrap":
            stage = CURRENT_STAGE
        print(f"[post-install] Manager {stage} failed: {error}", file=sys.stderr)
        traceback.print_exc()
        write_status("failed", "manager", stage, error)
        return 1
    finally:
        core.manager_funcs.run_script = original_run_script


def manager_files_path():
    try:
        if COMFY_PATH not in sys.path:
            sys.path.insert(0, COMFY_PATH)
        import folder_paths

        user_path = os.path.abspath(folder_paths.get_user_directory())
        if hasattr(folder_paths, "get_system_user_directory"):
            return os.path.join(user_path, "__manager")
        return os.path.join(user_path, "default", "ComfyUI-Manager")
    except Exception:
        modern = os.path.join(COMFY_PATH, "user", "__manager")
        legacy = os.path.join(COMFY_PATH, "user", "default", "ComfyUI-Manager")
        return modern if os.path.isdir(modern) else legacy


MANAGER_FILES = manager_files_path()
PIP_BLACKLIST = {"torch", "torchaudio", "torchsde", "torchvision"}
DOWNGRADE_BLACKLIST = {
    "torch",
    "torchaudio",
    "torchsde",
    "torchvision",
    "transformers",
    "safetensors",
    "kornia",
}
PIP_OVERRIDES = {}
USE_UV = False
PIP_COMMAND = None


def load_manager_policy():
    global USE_UV, PIP_OVERRIDES
    config_path = os.path.join(MANAGER_FILES, "config.ini")
    config = configparser.ConfigParser(strict=False)
    try:
        config.read(config_path)
        defaults = config["default"]
        USE_UV = defaults.get("use_uv", "false").lower() == "true"
        extra = defaults.get("downgrade_blacklist", "")
        DOWNGRADE_BLACKLIST.update(item.strip() for item in extra.split(",") if item.strip())
    except Exception:
        pass

    override_path = os.path.join(MANAGER_FILES, "pip_overrides.json")
    try:
        with open(override_path, "r", encoding="utf-8", errors="ignore") as stream:
            value = json.load(stream)
            if isinstance(value, dict):
                PIP_OVERRIDES = value
    except Exception:
        pass

    blacklist_path = os.path.join(MANAGER_FILES, "pip_blacklist.list")
    try:
        with open(blacklist_path, "r", encoding="utf-8", errors="ignore") as stream:
            PIP_BLACKLIST.update(line.strip() for line in stream if line.strip())
    except Exception:
        pass


def run_quiet(command):
    return subprocess.run(
        command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=False
    ).returncode == 0


def pip_command():
    global PIP_COMMAND
    if PIP_COMMAND is not None:
        return PIP_COMMAND[:]
    embedded = "python_embeded" in sys.executable
    isolation = ["-s"] if embedded else []
    normal = [sys.executable] + isolation + ["-m", "pip"]
    module_uv = [sys.executable] + isolation + ["-m", "uv", "pip"]
    if not USE_UV and run_quiet(normal + ["--version"]):
        PIP_COMMAND = normal
    elif run_quiet(module_uv[:-1] + ["--version"]):
        PIP_COMMAND = module_uv
    elif shutil.which("uv"):
        PIP_COMMAND = ["uv", "pip"]
    else:
        raise RuntimeError("Neither python -m pip nor uv is available")
    return PIP_COMMAND[:]


def installed_packages():
    try:
        output = subprocess.check_output(
            pip_command() + ["list"], text=True, stderr=subprocess.DEVNULL
        )
    except Exception:
        return {}
    packages = {}
    for line in output.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] != "Package" and not parts[0].startswith("-"):
            packages[parts[0].lower().replace("-", "_")] = parts[1]
    return packages


def version_tuple(value):
    numbers = re.findall(r"\d+", str(value))
    return tuple(int(number) for number in numbers[:3]) + (0,) * max(0, 3 - len(numbers))


def requirement_name(requirement):
    match = re.match(r"\s*([A-Za-z0-9_.+-]+)", requirement)
    return match.group(1).lower().replace("-", "_") if match else ""


def protected_requirement(requirement):
    name = requirement_name(requirement)
    canonical_blacklist = {item.lower().replace("-", "_") for item in PIP_BLACKLIST}
    if name in canonical_blacklist:
        print(f"[post-install] skipping protected pip package: {name}")
        return True
    canonical_downgrade = {
        item.lower().replace("-", "_") for item in DOWNGRADE_BLACKLIST
    }
    if name not in canonical_downgrade:
        return False
    current = installed_packages().get(name)
    if current is None:
        return False
    match = re.search(r"(<=|==|<|~=)\s*([0-9A-Za-z.+-]+)", requirement)
    if match and version_tuple(current) >= version_tuple(match.group(2)):
        print(f"[post-install] refusing downgrade of {name} ({current})")
        return True
    return not re.search(r"(>=|>)\s*([0-9A-Za-z.+-]+)", requirement)


def robust_lines(path):
    with open(path, "rb") as stream:
        data = stream.read()
    encodings = ["utf-8", locale.getpreferredencoding(False), "latin-1"]
    for encoding in dict.fromkeys(encodings):
        try:
            return data.decode(encoding).splitlines()
        except (UnicodeDecodeError, LookupError):
            pass
    return []


def requirement_args(requirement):
    if "--index-url" in requirement:
        package, index = requirement.split("--index-url", 1)
        return [package.strip(), "--index-url", index.strip()]
    return [requirement]


TORCH_VERSIONS = {
    "2.7.0": ("0.22.0", "2.7.0"),
    "2.6.0": ("0.21.0", "2.6.0"),
    "2.5.1": ("0.20.0", "2.5.0"),
    "2.5.0": ("0.20.0", "2.5.0"),
    "2.4.1": ("0.19.1", "2.4.1"),
    "2.4.0": ("0.19.0", "2.4.0"),
    "2.3.1": ("0.18.1", "2.3.1"),
    "2.3.0": ("0.18.0", "2.3.0"),
    "2.2.2": ("0.17.2", "2.2.2"),
    "2.2.1": ("0.17.1", "2.2.1"),
    "2.2.0": ("0.17.0", "2.2.0"),
    "2.1.2": ("0.16.2", "2.1.2"),
    "2.1.1": ("0.16.1", "2.1.1"),
    "2.1.0": ("0.16.0", "2.1.0"),
    "2.0.1": ("0.15.2", "2.0.1"),
    "2.0.0": ("0.15.1", "2.0.0"),
}


def run_repair(command):
    subprocess.check_output(command, text=True, stderr=subprocess.STDOUT)


def pip_fixer(previous):
    current = installed_packages()
    try:
        if "comfy" in current:
            run_repair(pip_command() + ["uninstall", "-y", "comfy"])
            print("[post-install] removed conflicting 'comfy' package")
    except Exception as error:
        print(f"[post-install] failed to remove 'comfy': {error}", file=sys.stderr)

    try:
        torch_names = ("torch", "torchvision", "torchaudio")
        if all(name in previous for name in torch_names) and any(
            previous[name] != current.get(name) for name in torch_names
        ):
            base, _, platform_tag = previous["torch"].partition("+")
            pair = TORCH_VERSIONS.get(base)
            if not platform_tag:
                run_repair(
                    pip_command()
                    + ["install", "--force", "torch", "torchvision", "torchaudio"]
                )
            elif pair:
                torchvision, torchaudio = pair
                index = f"https://download.pytorch.org/whl/{platform_tag}"
                run_repair(
                    pip_command()
                    + [
                        "install",
                        f"torch=={base}",
                        f"torchvision=={torchvision}",
                        f"torchaudio=={torchaudio}",
                        "--index-url",
                        index,
                    ]
                )
            else:
                index = f"https://download.pytorch.org/whl/nightly/{platform_tag}"
                run_repair(
                    pip_command()
                    + [
                        "install",
                        "--pre",
                        "torch",
                        "torchvision",
                        "torchaudio",
                        "--index-url",
                        index,
                    ]
                )
    except Exception as error:
        print(f"[post-install] failed to restore PyTorch: {error}", file=sys.stderr)

    try:
        opencv = [
            "opencv_contrib_python",
            "opencv_contrib_python_headless",
            "opencv_python",
            "opencv_python_headless",
        ]
        versions = [current[name] for name in opencv if name in current]
        if versions:
            newest = max(versions, key=version_tuple)
            for name in opencv:
                if name in current and current[name] != newest:
                    run_repair(pip_command() + ["install", f"{name.replace('_', '-')}=={newest}"])
    except Exception as error:
        print(f"[post-install] failed to align OpenCV: {error}", file=sys.stderr)

    try:
        current = installed_packages()
        if "comfyui_frontend_package" not in current:
            root_requirements = os.path.join(COMFY_PATH, "requirements.txt")
            frontend = next(
                (
                    line.strip()
                    for line in robust_lines(root_requirements)
                    if line.startswith("comfyui-frontend-package")
                ),
                None,
            )
            if frontend:
                run_repair(pip_command() + ["install", frontend])
    except Exception as error:
        print(f"[post-install] failed to restore frontend: {error}", file=sys.stderr)

    auto_fix = os.path.join(MANAGER_FILES, "pip_auto_fix.list")
    if os.path.isfile(auto_fix):
        for line in robust_lines(auto_fix):
            try:
                tokens = shlex.split(line)
                if not tokens:
                    continue
                name = requirement_name(tokens[0])
                current = installed_packages().get(name)
                match = re.search(r"(==|>=|<=|!=|~=|>|<)([0-9A-Za-z.+-]+)", tokens[0])
                satisfied = current is not None and match is None
                if current is not None and match:
                    left, right = version_tuple(current), version_tuple(match.group(2))
                    operation = match.group(1)
                    satisfied = {
                        "==": left == right,
                        ">=": left >= right,
                        "<=": left <= right,
                        "!=": left != right,
                        ">": left > right,
                        "<": left < right,
                        "~=": left >= right,
                    }[operation]
                if not satisfied:
                    run_repair(pip_command() + ["install"] + tokens)
            except Exception as error:
                print(f"[post-install] auto-fix failed for {line!r}: {error}", file=sys.stderr)


def run_builtin():
    global CURRENT_STAGE
    write_status("builtin_started", "builtin", "bootstrap")
    print("[post-install] using built-in ComfyUI-Manager compatibility backend")
    load_manager_policy()
    outer_previous = installed_packages()
    requirements = os.path.join(REPO_PATH, "requirements.txt")
    try:
        if os.path.isfile(requirements):
            CURRENT_STAGE = "requirements"
            inner_previous = installed_packages()
            processed = set()
            for original in robust_lines(requirements):
                requirement = original.strip()
                if not requirement or requirement.startswith("#") or requirement in processed:
                    continue
                processed.add(requirement)
                remapped = str(PIP_OVERRIDES.get(requirement, requirement))
                clean = remapped.split("#", 1)[0].strip()
                if not clean or protected_requirement(clean):
                    continue
                command = pip_command() + ["install"] + requirement_args(clean)
                print(f"[post-install] EXECUTE => {command}")
                subprocess.run(command, cwd=REPO_PATH, check=True)
            pip_fixer(inner_previous)

        install_script = os.path.join(REPO_PATH, "install.py")
        if os.path.isfile(install_script):
            CURRENT_STAGE = "install_script"
            command = [sys.executable, "install.py"]
            print(f"[post-install] EXECUTE => {command}")
            subprocess.run(command, cwd=REPO_PATH, check=True)

        CURRENT_STAGE = "repair"
        pip_fixer(outer_previous)
        write_status("completed", "builtin", "complete")
        return 0
    except BaseException as error:
        stage = command_stage(getattr(error, "cmd", []))
        if stage == "repair":
            stage = CURRENT_STAGE
        print(f"[post-install] built-in {stage} failed: {error}", file=sys.stderr)
        traceback.print_exc()
        write_status("failed", "builtin", stage, error)
        return 1


def main():
    os.environ.setdefault("COMFYUI_PATH", COMFY_PATH)
    os.environ.setdefault("COMFYUI_FOLDERS_BASE_PATH", COMFY_PATH)
    interpreter_dir = os.path.dirname(sys.executable)
    os.environ["PATH"] = interpreter_dir + os.pathsep + os.environ.get("PATH", "")

    manager = find_manager()
    if manager is not None:
        result = try_manager(manager)
        if result is not None:
            return result
        print("[post-install] falling back before Manager performed node work")
    return run_builtin()


raise SystemExit(main())
