use crate::cli::Shell;
use color_eyre::Result;

pub fn run(shell: Shell) -> Result<()> {
    print!("{}", integration(shell));
    Ok(())
}

fn integration(shell: Shell) -> &'static str {
    match shell {
        Shell::Sh => POSIX_SH,
        Shell::Bash => BASH,
        Shell::Zsh => ZSH,
        Shell::Fish => FISH,
        Shell::Nu => NU,
        Shell::Powershell => POWERSHELL,
    }
}

const POSIX_SH: &str = r#"aix() {
  if [ "${1-}" = "use" ]; then
    shift
    __aix_use_code=$(command aix use --shell sh "$@")
    __aix_use_status=$?
    if [ "$__aix_use_status" -eq 0 ]; then
      eval "$__aix_use_code"
      __aix_use_status=$?
    fi
    set -- "$__aix_use_status"
    unset __aix_use_code __aix_use_status
    return "$1"
  fi
  command aix "$@"
}
"#;

const BASH: &str = r#"aix() {
  if [ "${1-}" = "use" ]; then
    shift
    __aix_use_code=$(command aix use --shell bash "$@")
    __aix_use_status=$?
    if [ "$__aix_use_status" -eq 0 ]; then
      eval "$__aix_use_code"
      __aix_use_status=$?
    fi
    set -- "$__aix_use_status"
    unset __aix_use_code __aix_use_status
    return "$1"
  fi
  command aix "$@"
}
"#;

const ZSH: &str = r#"aix() {
  if [ "${1-}" = "use" ]; then
    shift
    __aix_use_code=$(command aix use --shell zsh "$@")
    __aix_use_status=$?
    if [ "$__aix_use_status" -eq 0 ]; then
      eval "$__aix_use_code"
      __aix_use_status=$?
    fi
    set -- "$__aix_use_status"
    unset __aix_use_code __aix_use_status
    return "$1"
  fi
  command aix "$@"
}
"#;

const FISH: &str = r#"function aix
    if test "$argv[1]" = use
        set --local use_args $argv[2..-1]
        set --local use_code (command aix use --shell fish $use_args)
        or return $status
        eval $use_code
    else
        command aix $argv
    end
end
"#;

const NU: &str = r#"def --env --wrapped aix [...args] {
    if ($args | is-empty) {
        ^aix ...$args
    } else if ($args | first) == "use" {
        let result = (^aix use --format json ...($args | skip 1) | complete)
        if $result.exit_code != 0 {
            print -e $result.stderr
            $env.LAST_EXIT_CODE = $result.exit_code
            return
        }
        let selection = ($result.stdout | from json)
        if $selection.AIX_PROFILE == null {
            hide-env --ignore-errors AIX_PROFILE
        } else {
            $env.AIX_PROFILE = $selection.AIX_PROFILE
        }
    } else {
        ^aix ...$args
    }
}
"#;

const POWERSHELL: &str = r#"function aix {
    if ($args.Count -gt 0 -and $args[0] -eq 'use') {
        $useArgs = @($args | Select-Object -Skip 1)
        $aixExecutable = (Get-Command aix -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $useScript = & $aixExecutable use --shell powershell @useArgs
        if ($LASTEXITCODE -ne 0) { return }
        Invoke-Expression ($useScript -join [Environment]::NewLine)
    } else {
        $aixExecutable = (Get-Command aix -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        & $aixExecutable @args
    }
}
"#;
