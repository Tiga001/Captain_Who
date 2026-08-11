use super::*;

#[test]
fn external_read_risk_uses_the_protocol_wire_value() {
    assert_eq!(
        serde_json::to_value(CommandRiskClass::ExternalRead).unwrap(),
        serde_json::json!("external_read")
    );
}

#[test]
fn policy_and_tool_preparation_share_the_same_command_character_limit() {
    let at_limit = format!("echo {}", "x".repeat(MAX_COMMAND_CHARS - "echo ".len()));
    assert_ne!(
        evaluate_command_policy(
            &at_limit,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        )
        .code,
        "command.malformed.too_long"
    );

    let above_limit = format!("echo {}", "x".repeat(MAX_COMMAND_CHARS + 1));
    let evaluation = evaluate_command_policy(
        &above_limit,
        AgentCommandSafetyPolicy::FullAccess,
        CommandAuthorizationSource::ExplicitUser,
    );
    assert_eq!(evaluation.decision, CommandPolicyDecision::Deny);
    assert_eq!(evaluation.code, "command.malformed.too_long");
}

#[test]
fn rejects_cwd_outside_workspace() {
    let workspace = TestWorkspace::new();
    let error = resolve_command_cwd(
        Some(&workspace.path),
        Some("../outside"),
        AgentWritePermission::WorkspaceOnly,
    )
    .unwrap_err();

    assert!(error.contains(".."));
}

#[test]
fn rejects_absolute_cwd_outside_workspace_without_full_write() {
    let workspace = TestWorkspace::new();
    let outside = TestWorkspace::new();

    let error = resolve_command_cwd(
        Some(&workspace.path),
        Some(&outside.path.to_string_lossy()),
        AgentWritePermission::WorkspaceOnly,
    )
    .unwrap_err();

    assert!(error.contains("write=all"));
}

#[test]
fn allows_absolute_cwd_outside_workspace_with_full_write() {
    let workspace = TestWorkspace::new();
    let outside = TestWorkspace::new();

    let resolved = resolve_command_cwd(
        Some(&workspace.path),
        Some(&outside.path.to_string_lossy()),
        AgentWritePermission::All,
    )
    .unwrap();

    assert_eq!(resolved, outside.path);
}

#[test]
fn guarded_automatic_policy_routes_risky_commands_to_explicit_approval() {
    for command in [
        "pip install openpyxl",
        "pip3 install openpyxl 2>&1",
        "printf hello > result.txt",
        "git reset --hard",
        "python3 -c 'print(1)'",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
    }
}

#[test]
fn full_access_automatic_and_explicit_user_allow_non_catastrophic_risk() {
    for command in [
        "pip install openpyxl",
        "pip3 install openpyxl 2>&1",
        "printf hello > result.txt",
        "git reset --hard",
        "python3 -c 'print(1)'",
    ] {
        for (policy, source) in [
            (
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::Automatic,
            ),
            (
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::ExplicitUser,
            ),
        ] {
            assert_eq!(
                evaluate_command_policy(command, policy, source).decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }
    }
}

#[test]
fn catastrophic_and_unsupported_commands_are_denied_for_every_authorization() {
    for command in [
        "rm -rf /",
        "sudo whoami",
        "pkexec whoami",
        "run0 whoami",
        "mkfs.ext4 /dev/disk9",
        "dd if=/dev/zero of=/dev/disk9",
        "vim file.txt",
        "echo 'unterminated",
    ] {
        for (policy, source) in [
            (
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            ),
            (
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::Automatic,
            ),
            (
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::ExplicitUser,
            ),
        ] {
            assert_eq!(
                evaluate_command_policy(command, policy, source).decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }
}

#[test]
fn guarded_automatic_keeps_only_statically_read_only_commands_automatic() {
    for command in [
        "git remote -v",
        "pip list",
        "cargo --version",
        "true && rg policy crates",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow,
            "{command}"
        );
    }
}

#[test]
fn shell_reserved_function_and_group_syntax_is_always_denied() {
    for command in [
        "! sudo whoami",
        "{ sudo whoami; }",
        "if true; then sudo whoami; fi",
        "function elevate { sudo whoami; }",
        "elevate() { sudo whoami; }",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::Deny,
            "{command}"
        );
        assert!(
            evaluation.findings.iter().any(|finding| {
                finding.code == "command.unsupported.shell_reserved_syntax"
                    || finding.code == "command.unsupported.shell_group"
            }),
            "{command}: {:?}",
            evaluation.findings
        );
    }
}

#[test]
fn guarded_automatic_does_not_trust_explicit_program_paths_or_environment_overrides() {
    for command in [
        "./ls",
        "/bin/ls",
        "tools/ls",
        "./busybox ls",
        "/tmp/busybox ls",
        "./command ls",
        "/tmp/builtin ls",
        "PATH=/tmp ls",
        "LD_PRELOAD=./shim.so ls",
        "DYLD_INSERT_LIBRARIES=./shim.dylib ls",
        "GIT_EXTERNAL_DIFF=./diff-helper git status",
        "env -u PATH ls",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}: {:?}",
            evaluation.findings
        );
        assert!(evaluation.findings.iter().any(|finding| {
            matches!(
                finding.code.as_str(),
                "command.risk.explicit_program_path" | "command.risk.environment_override"
            )
        }));
    }

    assert_eq!(
        evaluate_command_policy(
            "ls",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        )
        .decision,
        CommandPolicyDecision::Allow
    );

    let busybox_install = evaluate_command_policy(
        "busybox --install ls",
        AgentCommandSafetyPolicy::Guarded,
        CommandAuthorizationSource::Automatic,
    );
    assert_eq!(
        busybox_install.decision,
        CommandPolicyDecision::RequireExplicitApproval
    );
    assert!(busybox_install
        .findings
        .iter()
        .any(|finding| finding.risk == CommandRiskClass::Unknown));
}

#[test]
fn guarded_automatic_routes_dynamic_arguments_and_environment_reads_to_approval() {
    for command in [
        "echo $SECRET",
        "cat $HOME/.config/token",
        "cat ${HOME}/.config/token",
        "cat $(printf /etc/passwd)",
        "cat `printf /etc/passwd`",
        "env",
        "printenv",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}: {:?}",
            evaluation.findings
        );
    }
}

#[test]
fn read_only_commands_with_execution_hooks_are_not_automatically_trusted() {
    for command in [
        "rg --pre helper pattern .",
        "rg --pre=helper pattern .",
        "rg --pre-glob '*.md' pattern .",
        "sort --compress-program=gzip input.txt",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::Deny,
            "{command}"
        );
    }

    for command in [
        "git diff",
        "git show HEAD",
        "git log -1",
        "git show --ext-diff HEAD",
        "git log --textconv",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}: {:?}",
            evaluation.findings
        );
        assert!(evaluation
            .findings
            .iter()
            .any(|finding| finding.code == "command.risk.git_external_diff"));
    }
}

#[test]
fn workspace_only_reads_route_obvious_external_paths_to_approval() {
    let workspace_only = policy_permissions(
        AgentReadPermission::WorkspaceOnly,
        AgentCommandSafetyPolicy::Guarded,
    );
    for command in [
        "cat /etc/passwd",
        "head ~/.ssh/id_ed25519",
        "tail ../outside.log",
        "rg token @home",
        "grep -f/etc/passwd pattern local.txt",
        "rg -f/etc/patterns pattern .",
        "stat file:///etc/passwd",
    ] {
        let evaluation = evaluate_command_policy_with_permissions(
            command,
            workspace_only,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}: {:?}",
            evaluation.findings
        );
        assert!(evaluation
            .findings
            .iter()
            .any(|finding| finding.code == "command.scope.external_read"));
    }

    assert_eq!(
        evaluate_command_policy_with_permissions(
            "cat local.txt",
            workspace_only,
            CommandAuthorizationSource::Automatic,
        )
        .decision,
        CommandPolicyDecision::Allow
    );
    assert_eq!(
        evaluate_command_policy_with_permissions(
            "cat /etc/passwd",
            policy_permissions(AgentReadPermission::All, AgentCommandSafetyPolicy::Guarded),
            CommandAuthorizationSource::Automatic,
        )
        .decision,
        CommandPolicyDecision::Allow
    );
}

#[test]
fn recursive_destruction_of_platform_sensitive_roots_is_always_denied() {
    for target in [
        "/Volumes",
        "/Volumes/Backup",
        "/private/etc",
        "/dev",
        "/boot",
        "/lib",
        "/root",
        "/proc",
        "/sys",
        "/mnt/data",
        "/media/user/drive",
    ] {
        let command = format!("rm -rf {target}");
        let evaluation = evaluate_command_policy(
            &command,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::Deny,
            "{command}: {:?}",
            evaluation.findings
        );
        assert_eq!(
            evaluation.code, "command.catastrophic.filesystem_root",
            "{command}"
        );
    }
}

#[test]
fn guarded_automatic_allows_only_explicitly_allowlisted_workspace_tasks() {
    for command in ["cargo test", "pnpm lint", "npm run build", "make test"] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow,
            "{command}"
        );
    }

    for command in [
        "cargo run",
        "pnpm deploy",
        "npm run arbitrary-script",
        "make deploy",
        "npm publish",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
    }
}

#[test]
fn guarded_automatic_workspace_tasks_use_a_closed_runner_grammar() {
    for command in [
        "cargo test --manifest-path /tmp/evil/Cargo.toml",
        "cargo test --config build.rustc-wrapper=evil",
        "cargo test --target /tmp/evil.json",
        "cargo test --target=../evil.json",
        "make test -f /tmp/evil.mk",
        "make test --eval='test:; @echo pwn'",
        "npm test --prefix /tmp/evil",
        "yarn test --cwd /tmp/evil",
        "pnpm test --dir /tmp/evil",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow,
            "{command}"
        );
    }

    for command in [
        "cargo test -p mycopilot-core --locked -- --nocapture",
        "npm run build",
        "pnpm lint",
        "make check",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow,
            "{command}"
        );
    }
}

#[test]
fn read_only_closed_grammar_blocks_execution_and_write_option_bypasses() {
    for command in [
        "rg --hostname-bin=/tmp/evil --hyperlink-format='file://{host}{path}' pattern .",
        "rg -z pattern .",
        "rg --search-zip pattern .",
        "find . -ok sh -c 'echo pwn' \\;",
        "find . -okdir sh -c 'echo pwn' \\;",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            )
            .decision,
            CommandPolicyDecision::Deny,
            "{command}"
        );
    }

    for command in [
        "sort --output /tmp/output input",
        "sort -o/tmp/output input",
        "sort --temporary-directory /tmp input",
        "sort -T/tmp input",
        "find . -fprint0 /tmp/output",
        "file -C -m ./magic",
        "file --compile --magic-file ./magic",
        "file -z archive.gz",
        "date -f /etc/passwd",
        "cargo metadata",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
    }

    assert_eq!(
        evaluate_command_policy(
            "pip list --outdated",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        )
        .risk_level,
        AgentCommandRiskLevel::Network
    );
}

#[test]
fn git_read_grammar_does_not_auto_run_hooks_or_accept_mutating_suffixes() {
    for command in [
        "git status --short",
        "git diff",
        "git log -p",
        "git show HEAD",
        "git log --output=/tmp/log",
        "git remote -v add origin https://example.invalid/repo",
        "git branch -a -D victim",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
    }
    for command in [
        "git remote",
        "git remote -v",
        "git remote get-url origin",
        "git branch",
        "git branch --show-current",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow,
            "{command}"
        );
    }
}

#[test]
fn environment_equivalents_and_shell_path_expansion_require_approval() {
    for command in [
        "jq -n env",
        "jq -n '$ENV'",
        "jq 'include \"helpers\"; run' data.json",
        "cat {../secret,local}",
        "cat .?/.ssh/id_rsa",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
    }
}

#[test]
fn direct_system_control_and_recursive_symlink_traversal_are_always_denied() {
    for command in [
        "dd if=/dev/zero of=/dev/mem",
        "tee /dev/kmem",
        "echo b > /proc/sysrq-trigger",
        "printf 1 > /sys/kernel/control",
        "systemctl reboot",
        "systemctl isolate reboot.target",
        "systemctl --no-block start poweroff.target",
        "systemctl isolate runlevel6.target",
        "systemctl isolate runlevel0.target",
        "systemctl --no-block isolate multi-user.target",
        "systemctl restart runlevel6.target",
        "systemctl start ctrl-alt-del.target",
        "systemctl enable --now reboot.target",
        "systemctl --now reenable runlevel6.target",
        "systemctl preset runlevel0.target --now",
        "systemctl --now preset ctrl-alt-del.target",
        "systemctl link --now reboot.target",
        "systemctl reload-or-restart poweroff.target",
        "systemctl try-reload-or-restart reboot.target",
        "systemctl reload-or-try-restart ctrl-alt-del.target",
        "systemctl start 'reboot.*'",
        "systemctl start '*.target'",
        "systemctl restart 'runlevel?.target'",
        "systemctl soft-reboot",
        "init 0",
        "telinit 6",
        "launchctl reboot system",
        "kill -9 1",
        "kill -9 0001",
        "kill -9 -1",
        "kill -9 -0001",
        "kill -9 '1'</dev/null",
        "kill -9 \"0001\"</dev/null",
        "kill -9 \\1</dev/null",
        "chmod -RL 000 link",
        "chown -RH root link",
        "rm -rf /var/lib",
        "rm -rf /private/var/db",
    ] {
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            )
            .decision,
            CommandPolicyDecision::Deny,
            "{command}"
        );
    }
}
