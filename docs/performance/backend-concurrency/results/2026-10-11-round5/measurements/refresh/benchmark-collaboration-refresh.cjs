const fs = require('node:fs')
const path = require('node:path')
const cp = require('node:child_process')
const crypto = require('node:crypto')
const ts = require('typescript')
const baseline = 'df3e6f04'
const file = 'src/renderer/src/features/agentCollaboration/collaborationStore.ts'
const dir = __dirname
const tick = () => new Promise(resolve => setImmediate(resolve))
async function load(code, name) {
  const isolated = code.replace(/import \{ hostCollaborationDataSource, type CollaborationDataSource \} from '.\/collaborationClient'/, 'const hostCollaborationDataSource: any = undefined')
  const js = ts.transpileModule(isolated, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText
  return (await import('data:text/javascript;base64,' + Buffer.from(js + '\n//' + name).toString('base64'))).CollaborationStore
}
const tree = n => ({ schemaVersion: 1, workspaceId:'fixture-project', projectId:'fixture-project', rootAgentId:'fixture-root', rootConversationId:'fixture-conversation', agents:Array.from({length:32},(_,i)=>({agentId:'child-'+i,parentAgentId:'fixture-root',taskName:'Synthetic task '+i})),lastSequence:n })
const event = (n, kind='turn_updated') => ({schemaVersion:3,eventId:'event-'+n,sequence:n,workspaceId:'fixture-project',projectId:'fixture-project',rootAgentId:'fixture-root',rootConversationId:'fixture-conversation',agentId:'child-'+(n%32),conversationId:'child-conversation-'+(n%32),turnId:null,runId:null,messageId:null,kind,resourceRevision:n,activities:[],occurredAt:n})
const clone = x => JSON.parse(JSON.stringify(x))
async function run(Store, version, scenario) {
  let n=1, active=0, peak=0, notify, resync, blocked=false, release
  let gate = new Promise(resolve => { release=resolve })
  const events=[event(1,'agent_created')]
  const metrics={getTree:0,listEvents:0,approvalList:0,publications:0}
  const source={
    async getTree(){metrics.getTree++; active++;peak=Math.max(peak,active); try { if(blocked) await gate; return clone(tree(n)) } finally {active--} },
    async listEvents({afterSequence}){metrics.listEvents++;active++;peak=Math.max(peak,active);try{return clone({schemaVersion:1,rootAgentId:'fixture-root',rootConversationId:'fixture-conversation',events:events.slice(afterSequence),lastSequence:n,hasMore:false})}finally{active--}},
    subscribe(handler){notify=handler;return ()=>{}},subscribeResync(handler){resync=handler;return ()=>{}}
  }
  const store=new Store('fixture-conversation', source)
  store.start();await tick()
  let previousKey=version==='before'?String(store.getSnapshot().tree.lastSequence):`${store.getSnapshot().hydrationRevision}:${store.getSnapshot().approvalInvalidationSequence}`
  store.subscribe(()=>{
    metrics.publications++
    const snapshot=store.getSnapshot()
    const key=version==='before'?String(snapshot.tree?.lastSequence??0):`${snapshot.hydrationRevision}:${snapshot.approvalInvalidationSequence}`
    if(key!==previousKey){previousKey=key;metrics.approvalList++;clone({schemaVersion:1,approvals:Array.from({length:32},(_,i)=>({approvalId:'approval-'+i,status:'pending',rootConversationId:'fixture-conversation',sourceAgentId:'child-'+i,action:{type:'command',command:{command:'synthetic command'}}}))})}
  })
  for(const key of Object.keys(metrics))metrics[key]=0
  peak=0
  const cpu=process.cpuUsage(), start=performance.now()
  if(scenario==='resync_burst_40') {
    blocked=true
    for(let i=0;i<40;i++){resync();await tick()}
    blocked=false;release();await tick();await tick()
  } else {
    for(let i=0;i<100;i++) {
      n++;const kind=i%10===9?(i%20===19?'approval_updated':'approval_projected'):['turn_updated','mailbox_enqueued','wake_updated'][i%3]
      const e=event(n,kind);events.push(e);notify(e);await tick()
    }
  }
  const elapsedMs=performance.now()-start, used=process.cpuUsage(cpu)
  const snapshot=store.getSnapshot()
  if(snapshot.loading || snapshot.error || snapshot.tree?.lastSequence!==n)throw new Error('Final snapshot mismatch')
  const result={version,scenario,...metrics,peakConcurrentReads:peak,cpuUserMs:used.user/1000,cpuSystemMs:used.system/1000,cpuTotalMs:(used.user+used.system)/1000,elapsedMs,finalSequence:snapshot.tree.lastSequence,finalActivityCount:snapshot.activities.length,finalError:snapshot.error}
  store.destroy()
  return result
}
;(async()=>{
  const oldSource=cp.execFileSync('git',['show',baseline+':'+file],{encoding:'utf8'})
  const newSource=fs.readFileSync(file,'utf8')
  const Before=await load(oldSource,'before'), After=await load(newSource,'after')
  const samples=[]
  for(let repetition=-2;repetition<20;repetition++) for(const scenario of ['resync_burst_40','mixed_durable_100']) for(const [version,Store] of repetition%2===0?[['before',Before],['after',After]]:[['after',After],['before',Before]]) {
    const sample=await run(Store,version,scenario)
    if(repetition>=0)samples.push({repetition,...sample})
  }
  const median=xs=>{const a=[...xs].sort((a,b)=>a-b);return(a[Math.floor((a.length-1)/2)]+a[Math.floor(a.length/2)])/2}
  const summary=[]
  for(const scenario of ['resync_burst_40','mixed_durable_100'])for(const version of ['before','after']) {
    const rows=samples.filter(x=>x.scenario===scenario&&x.version===version)
    const out={scenario,version,samples:rows.length}
    for(const key of ['getTree','listEvents','approvalList','publications','peakConcurrentReads','cpuTotalMs','elapsedMs','finalSequence'])out[key]=median(rows.map(x=>x[key]))
    summary.push(out)
  }
  const report={metadata:{baseline,currentStoreSha256:crypto.createHash('sha256').update(newSource).digest('hex'),node:process.version,scope:'Actual CollaborationStore implementation before/after with injected synthetic JSON transport; approval call count follows actual AppShell invalidation keys; CPU is Node process CPU for this fixture only, not real Core, browser or E2E latency.',warmups:2,samplesPerCase:20,realUserData:false,modelCalls:0},summary,samples}
  fs.writeFileSync(path.join(dir,'collaboration-refresh-results.json'),JSON.stringify(report,null,2)+'\n')
  console.log(JSON.stringify({metadata:report.metadata,summary},null,2))
})().catch(error=>{console.error(error);process.exitCode=1})
