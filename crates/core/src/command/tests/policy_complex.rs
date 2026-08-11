use super::*;

#[test]
fn every_compound_segment_is_evaluated() {
    for command in [
        "true && pip install openpyxl",
        "true; /usr/local/bin/pip3 install openpyxl",
        "printf ok | sh -c 'pip install openpyxl'",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}"
        );
        assert!(evaluation
            .findings
            .iter()
            .any(|finding| finding.risk == CommandRiskClass::PackageManagement));
    }

    let wrapped_redirect = evaluate_command_policy(
        "sh -c 'printf ok' > output.txt",
        AgentCommandSafetyPolicy::Guarded,
        CommandAuthorizationSource::Automatic,
    );
    assert_eq!(
        wrapped_redirect.decision,
        CommandPolicyDecision::RequireExplicitApproval
    );
    assert!(wrapped_redirect
        .findings
        .iter()
        .any(|finding| finding.risk == CommandRiskClass::DirectWrite));
}

#[test]
fn every_multiline_and_continued_segment_is_evaluated() {
    for command in [
        "true\npip install openpyxl",
        "true;\npip install openpyxl",
        "true &&\npip install openpyxl",
        "true ||\npip install openpyxl",
        "printf ok |\npip install openpyxl",
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
            .any(|finding| finding.risk == CommandRiskClass::PackageManagement));
    }

    for command in [
        "true\nrm -rf /",
        "true # the next line must still be analyzed\nrm -rf /",
        "true &&\nrm -rf /",
        "true ||\nrm -rf /",
        "printf ok |\nrm -rf /",
        "r\\\nm -rf /",
        "r\\\r\nm -rf /",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::Deny,
            "{command}: {:?}",
            evaluation.findings
        );
        assert!(evaluation
            .findings
            .iter()
            .any(|finding| finding.risk == CommandRiskClass::Catastrophic));
    }

    let trailing_semicolon = evaluate_command_policy(
        "true;",
        AgentCommandSafetyPolicy::FullAccess,
        CommandAuthorizationSource::ExplicitUser,
    );
    assert_eq!(trailing_semicolon.decision, CommandPolicyDecision::Deny);
    assert!(trailing_semicolon
        .findings
        .iter()
        .any(|finding| finding.code == "command.malformed.trailing_operator"));
}

#[test]
fn quoted_heredoc_is_data_but_all_executable_boundaries_remain_visible() {
    for command in [
        "python - <<'PY'\nprint('> /dev/disk9')\nprint('rm -rf /')\nPY",
        "node <<\"JS\"\nconsole.log('sudo whoami')\nJS",
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
        assert!(!evaluation
            .findings
            .iter()
            .any(|finding| matches!(finding.risk, CommandRiskClass::Catastrophic)));
    }

    let followed_by_command = evaluate_command_policy(
        "python - <<'PY'\nprint('safe data')\nPY\nrm -rf /",
        AgentCommandSafetyPolicy::FullAccess,
        CommandAuthorizationSource::ExplicitUser,
    );
    assert_eq!(followed_by_command.decision, CommandPolicyDecision::Deny);
    assert!(followed_by_command
        .findings
        .iter()
        .any(|finding| finding.risk == CommandRiskClass::Catastrophic));

    let pipeline_header = evaluate_command_policy(
        "python - <<'PY' |\ncat\nprint('safe data')\nPY",
        AgentCommandSafetyPolicy::Guarded,
        CommandAuthorizationSource::Automatic,
    );
    assert_eq!(
        pipeline_header.decision,
        CommandPolicyDecision::RequireExplicitApproval
    );
    assert!(pipeline_header
        .findings
        .iter()
        .any(|finding| finding.code == "command.risk.heredoc_interpreter"));
    assert!(pipeline_header
        .findings
        .iter()
        .any(|finding| finding.program == "cat"));
}

#[test]
fn unsafe_or_ambiguous_heredoc_forms_fail_closed() {
    for command in [
        "python - <<PY\nprint('unquoted')\nPY",
        "python - <<< 'print(1)'",
        "python - <<'A' <<'B'\nA\nB",
        "python - <<'PY'\nprint('missing exact terminator')\n PY",
        "sh <<'EOF'\nrm -rf /\nEOF",
        "env sh <<'EOF'\nrm -rf /\nEOF",
    ] {
        let evaluation = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        );
        assert_eq!(
            evaluation.decision,
            CommandPolicyDecision::Deny,
            "{command}: {:?}",
            evaluation.findings
        );
    }
}

#[test]
fn nested_catastrophic_commands_cannot_hide_in_shell_wrappers_or_substitutions() {
    for command in [
        "sh -c 'rm -rf /'",
        "bash -lc \"sudo whoami\"",
        "env -u TOKEN /usr/bin/sudo whoami",
        "command env -S 'sudo whoami'",
        "busybox env -S 'sudo whoami'",
        "echo $(rm -rf /)",
        "echo `sudo whoami`",
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
}

#[test]
fn opaque_wrappers_dynamic_programs_and_background_execution_are_always_denied() {
    for command in [
        "env -S 'sh -c \"sudo whoami\"'",
        "env -S\"sh -c 'sudo whoami'\" true",
        "$DIR/env -S 'printf ok'",
        "$DIR/busybox ls",
        "$DIR/command ls",
        "command $DIR/env -S 'printf ok'",
        "xargs sh -c 'sudo whoami'",
        "source ./script.sh",
        "time -o timing.txt sudo whoami",
        "timeout 5 sh -c 'echo ok'",
        "X=sudo $X whoami",
        "$(printf sudo) whoami",
        "make test & true",
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
    assert_eq!(
        evaluate_command_policy(
            "busybox rm -rf /",
            AgentCommandSafetyPolicy::FullAccess,
            CommandAuthorizationSource::ExplicitUser,
        )
        .decision,
        CommandPolicyDecision::Deny
    );
}

#[test]
fn explicit_shell_scripts_require_guarded_approval_but_are_trusted_after_authorization() {
    for command in [
        "sh ./skill-script.sh --check",
        "bash -e scripts/install.sh",
        "zsh -- ./scripts/verify.zsh",
        "fish -C 'echo preparing' ./scripts/verify.fish",
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
        assert_eq!(
            evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::ExplicitUser,
            )
            .decision,
            CommandPolicyDecision::Allow,
            "{command}"
        );
    }

    for command in [
        "bash --rcfile ./startup.bash -c 'echo ok'",
        "bash --init-file ./startup.bash -c 'echo ok'",
        "bash -lc 'echo ok'",
        "bash --debugger -c 'echo ok'",
        "bash -O extglob -c 'echo ok'",
        "zsh -c 'echo ok'",
        "fish -c 'echo ok'",
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
        "sh $SCRIPT",
        "bash ~/script.sh",
        "zsh scripts/*.zsh",
        "bash -i",
        "bash -ic 'echo interactive'",
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

#[test]
fn raw_devices_and_dynamic_redirection_targets_are_always_denied() {
    for command in [
        "cat > /dev/disk9",
        "cat > /tmp/../dev/disk9",
        "tee /dev/nvme0n1",
        "dd if=/dev/zero of=/tmp/../dev/disk9",
        "sort -o /dev/rdisk4 input.txt",
        "find . -fprint /dev/sda",
        "printf value > $TARGET",
        "printf value >&/dev/disk9",
        "printf value >>&/tmp/../dev/disk9",
        "printf value > /dev/d[i]sk9",
        "printf value > ~/output.txt",
        "printf value > /dev/{disk,rdisk}9",
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
        "printf value >&1",
        "printf value 2>&1",
        "dd if=/dev/zero of=/dev/null count=0",
    ] {
        assert_ne!(
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

    let ordinary_posix_redirect = evaluate_command_policy(
        "printf value >&output.txt",
        AgentCommandSafetyPolicy::Guarded,
        CommandAuthorizationSource::Automatic,
    );
    assert_eq!(
        ordinary_posix_redirect.decision,
        CommandPolicyDecision::RequireExplicitApproval
    );
    assert!(ordinary_posix_redirect
        .findings
        .iter()
        .any(|finding| finding.risk == CommandRiskClass::DirectWrite));
}

#[test]
fn device_tcp_and_udp_input_redirections_are_never_automatic() {
    for command in [
        "cat </dev/tcp/example.com/80",
        "head < /dev/udp/example.com/53",
        "cat <local.txt</dev/tcp/example.com/80",
    ] {
        let automatic = evaluate_command_policy(
            command,
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            automatic.decision,
            CommandPolicyDecision::RequireExplicitApproval,
            "{command}: {:?}",
            automatic.findings
        );
        assert_eq!(automatic.risk_level, AgentCommandRiskLevel::Network);
        assert!(automatic
            .findings
            .iter()
            .any(|finding| finding.code == "command.risk.network_redirection"));

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
}

#[test]
fn input_redirection_targets_participate_in_read_scope_and_syntax_policy() {
    let workspace_only = policy_permissions(
        AgentReadPermission::WorkspaceOnly,
        AgentCommandSafetyPolicy::Guarded,
    );
    for command in [
        "cat </etc/passwd",
        "cat <local.txt</etc/passwd",
        "grep x <../secret",
        "head <\"$HOME/.ssh/id_rsa\"",
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
        assert!(evaluation.findings.iter().any(|finding| {
            matches!(
                finding.code.as_str(),
                "command.scope.external_input_redirection"
                    | "command.risk.dynamic_input_redirection"
            )
        }));
    }

    assert_eq!(
        evaluate_command_policy_with_permissions(
            "cat <local.txt",
            workspace_only,
            CommandAuthorizationSource::Automatic,
        )
        .decision,
        CommandPolicyDecision::Allow
    );
    assert_eq!(
        evaluate_command_policy(
            "cat </etc/passwd",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        )
        .decision,
        CommandPolicyDecision::Allow
    );
    assert_eq!(
        evaluate_command_policy(
            "cat <>local.txt",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        )
        .decision,
        CommandPolicyDecision::RequireExplicitApproval
    );

    for command in [
        "cat <<EOF",
        "cat <<<secret",
        "cat <(printf secret)",
        "cat <&3",
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

#[test]
fn find_exec_and_opaque_recursive_targets_are_always_denied() {
    for command in [
        "find . -exec sh -c 'sudo whoami' \\;",
        "find . -execdir sudo whoami \\;",
        "find /[e]tc -delete",
        "find -H / -delete",
        "find -L /Users/example -delete",
        "rm -rf /[e]tc",
        "rm -rf target/*",
        "chmod -R 000 ~root",
        "chown -R user /{etc,var}",
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

#[test]
fn destructive_targets_are_lexically_resolved_against_cwd() {
    for command in ["rm -rf /tmp/../etc", "rm -rf /Users/example/*"] {
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

    for command in ["rm -rf ..", "rm -rf ../*", "chmod -R 755 .."] {
        assert_eq!(
            evaluate_command_policy_at(
                command,
                policy_permissions(
                    AgentReadPermission::All,
                    AgentCommandSafetyPolicy::FullAccess,
                ),
                CommandAuthorizationSource::ExplicitUser,
                None,
                Some(Path::new("/Users/example/project")),
            )
            .decision,
            CommandPolicyDecision::Deny,
            "{command}"
        );
    }
}
