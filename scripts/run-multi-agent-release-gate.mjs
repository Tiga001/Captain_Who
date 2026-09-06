/* eslint-disable @typescript-eslint/explicit-function-return-type -- Release runner inputs are validated at the process boundary. */

import { spawnSync } from 'node:child_process'
import os from 'node:os'
import process from 'node:process'

const FACTS_MINIMUM = 10_000
const RESTARTS_MINIMUM = 20
const SUBSCRIPTIONS_MINIMUM = 1_000

const facts = positiveInteger('MYCOPILOT_MULTI_AGENT_PROFILE_FACTS', FACTS_MINIMUM, FACTS_MINIMUM)
const restarts = positiveInteger(
  'MYCOPILOT_MULTI_AGENT_PROFILE_RESTARTS',
  RESTARTS_MINIMUM,
  RESTARTS_MINIMUM
)
const subscriptions = positiveInteger(
  'MYCOPILOT_MULTI_AGENT_PROFILE_SUBSCRIPTIONS',
  SUBSCRIPTIONS_MINIMUM,
  SUBSCRIPTIONS_MINIMUM
)

const profileOnly = process.argv.includes('--profile-only')
const smokeOnly = process.argv.includes('--smoke-only')
if (profileOnly && smokeOnly) fail('Choose at most one of --profile-only and --smoke-only.')

const environment = {
  ...process.env,
  MYCOPILOT_MULTI_AGENT_PROFILE_FACTS: String(facts),
  MYCOPILOT_MULTI_AGENT_PROFILE_RESTARTS: String(restarts),
  MYCOPILOT_MULTI_AGENT_PROFILE_SUBSCRIPTIONS: String(subscriptions)
}

console.log(
  `ROUND6_ENV ${JSON.stringify({
    arch: os.arch(),
    cpuCount: os.cpus().length,
    facts,
    node: process.version,
    platform: os.platform(),
    release: os.release(),
    restarts,
    subscriptions,
    totalMemoryBytes: os.totalmem()
  })}`
)

const profileSteps = [
  {
    label: 'persistence: tree 64/65, 10000 facts, four callers, twenty crash recoveries',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core',
      '--lib',
      'storage::service::child_agents::tests::model_recovery::release_profile_tree_mailbox_event_contention_and_restart_recovery',
      '--',
      '--ignored',
      '--exact',
      '--nocapture'
    ],
    resourceProfile: true
  },
  {
    label: 'dispatcher: one hundred Agents at global concurrency fifty',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'application::agent_dispatcher::tests::one_hundred_agents_respect_limit_fifty_fifo_and_leave_no_ghost_activity',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'context: five hundred logical turns reach a durable compaction boundary',
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'application::agent::tests::context_history::five_hundred_turn_context_compaction_release_profile',
      '--',
      '--ignored',
      '--exact',
      '--nocapture'
    ],
    expectedRustTests: 1,
    resourceProfile: true
  },
  {
    label: 'renderer: one thousand store and Host subscription lifecycles',
    command: [
      'pnpm',
      'exec',
      'vitest',
      'run',
      'src/renderer/src/features/agentCollaboration/collaborationStore.test.ts',
      'src/preload/AgentIpcBridge.test.ts'
    ]
  }
]

const smokeSteps = [
  {
    label: 'fault injection: terminal/result/outbox/parent Wake rollback and retry',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core',
      '--lib',
      'storage::agent_graph_repository::tests::result_recovery::terminal_result_faults_rollback_and_recover_exactly_once_after_restart',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'real deterministic six-tool Runtime/Host/server collaboration chain',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'application::agent::tests::collaboration_harness::fake_provider_drives_all_six_tools_through_runtime_host_and_server_services',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'restart: queued child executes without a new root Turn',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'application::agent::tests::collaboration_harness::process_start_dispatcher_recovers_a_queued_child_without_a_new_root_turn',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'wait domains: Command Session and Agent wait remain independent',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'application::agent::tests::command_sessions::real_registry_command_wait_and_agent_wait_are_isolated_without_shell',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'Approval: child continuation persists Waiting-to-Running before Runtime',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'application::agent::tests::pending_actions::child_approval_continuation_persists_waiting_to_running_before_runtime',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'authorization: every touched legacy child read/write bypass is rejected',
    expectedRustTests: 1,
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core-server',
      '--bin',
      'core-server',
      'transport::tests::collaboration_authorization::ordinary_user_rpc_cannot_read_or_mutate_a_child_conversation',
      '--',
      '--exact',
      '--nocapture'
    ]
  },
  {
    label: 'storage: canonical v40, reset refusal, and atomic fresh creation',
    command: [
      'cargo',
      'test',
      '--locked',
      '-p',
      'mycopilot-core',
      '--lib',
      'storage::migrations::tests'
    ]
  },
  {
    label: 'cross-language collaboration protocol fixture',
    command: ['cargo', 'test', '--locked', '-p', 'mycopilot-protocol-rs']
  },
  {
    label: 'AppShell: activity, Approval, observer, live stream, restart and root switching',
    command: [
      'pnpm',
      'exec',
      'vitest',
      'run',
      '--project',
      'browser',
      'src/renderer/src/app/__tests__/AppShellCollaborationScenario.browser.test.tsx',
      'src/renderer/src/features/chat/__tests__/ObserverConversationHook.browser.test.tsx',
      'src/renderer/src/features/rightSidebar/__tests__/AgentCenterRightSidebar.browser.test.tsx'
    ]
  }
]

const selectedSteps = profileOnly
  ? profileSteps
  : smokeOnly
    ? smokeSteps
    : [...profileSteps, ...smokeSteps]
const gateStarted = process.hrtime.bigint()
for (const step of selectedSteps) runStep(step)
const gateSeconds = Number(process.hrtime.bigint() - gateStarted) / 1_000_000_000
console.log(
  `ROUND6_GATE_RESULT steps=${selectedSteps.length} exit=0 wall_seconds=${gateSeconds.toFixed(3)}`
)

function runStep({ command, expectedRustTests, label, resourceProfile = false }) {
  const [program, ...args] = command
  const measured = resourceProfile ? measuredCommand(program, args) : { program, args }
  console.log(
    `\nROUND6_STEP_START label=${JSON.stringify(label)} command=${JSON.stringify(command)}`
  )
  const started = process.hrtime.bigint()
  const captureOutput = expectedRustTests !== undefined
  const result = spawnSync(measured.program, measured.args, {
    cwd: process.cwd(),
    encoding: captureOutput ? 'utf8' : undefined,
    env: environment,
    stdio: captureOutput ? ['inherit', 'pipe', 'pipe'] : 'inherit'
  })
  if (captureOutput) {
    process.stdout.write(result.stdout ?? '')
    process.stderr.write(result.stderr ?? '')
  }
  const seconds = Number(process.hrtime.bigint() - started) / 1_000_000_000
  if (result.error) fail(`${label}: ${result.error.message}`)
  if (result.status !== 0) fail(`${label}: exit ${result.status ?? 'signal'}`)
  if (
    expectedRustTests !== undefined &&
    !(result.stdout ?? '').includes(`test result: ok. ${expectedRustTests} passed;`)
  ) {
    fail(`${label}: expected exactly ${expectedRustTests} Rust test(s) to run.`)
  }
  console.log(
    `ROUND6_STEP_RESULT label=${JSON.stringify(label)} exit=0 wall_seconds=${seconds.toFixed(3)}`
  )
}

function measuredCommand(program, args) {
  if (process.platform === 'darwin') {
    return { program: '/usr/bin/time', args: ['-l', program, ...args] }
  }
  if (process.platform === 'linux') {
    return { program: '/usr/bin/time', args: ['-v', program, ...args] }
  }
  return { program, args }
}

function positiveInteger(name, fallback, minimum) {
  const raw = process.env[name]
  const value = raw === undefined ? fallback : Number(raw)
  if (!Number.isSafeInteger(value) || value < minimum) {
    fail(`${name} must be a safe integer >= ${minimum}; received ${raw ?? value}.`)
  }
  return value
}

function fail(message) {
  console.error(`ROUND6_GATE_FAILURE ${message}`)
  process.exit(1)
}
