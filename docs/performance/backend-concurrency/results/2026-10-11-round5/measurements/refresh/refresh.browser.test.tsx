import React from 'react'
import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { useCollaborationApprovals as beforeApproval } from './baseline-useCollaborationApprovals'
import { useObserverConversation as beforeObserver } from './baseline-useObserverConversation'
import { useCollaborationApprovals as afterApproval } from '../../../src/renderer/src/features/agentCollaboration/useCollaborationApprovals'
import { useObserverConversation as afterObserver } from '../../../src/renderer/src/features/agentCollaboration/useObserverConversation'
const clients=vi.hoisted(()=>({list:vi.fn(),load:vi.fn()}))
vi.mock('../../../src/renderer/src/features/agentCollaboration/collaborationClient',()=>({
listCollaborationApprovals: clients.list,decideCollaborationApproval:vi.fn(),
loadCollaborationObserverConversation:clients.load,onCollaborationObserverEvent:()=>()=>{}
}))
vi.mock('../../../src/renderer/src/features/agent/agentClient',()=>({onAgentEvent:()=>()=>{}}))
const snapshot={schemaVersion:1,agentId:'child',rootConversationId:'root',conversationId:'child-conversation',projectId:null,modelId:'synthetic',title:'synthetic',createdAt:1,updatedAt:2,messages:[{messageId:'message',role:'assistant',content:'same synthetic content',createdAt:2,status:'sent',inputOrigin:null,attachments:[],agentRunJson:null,uiStateJson:null}]}
function Approval({hook,seq}:any){const state=hook({rootConversationId:'root',invalidationSequence:seq});return <output>{state.loading?'loading':'settled'}</output>}
function Observer({hook,seq}:any){const state=hook({agentId:'child',rootAgentId:'root-agent',rootConversationId:'root',conversationId:'child-conversation',invalidationVersion:seq});return <output>{state.loading?'loading':state.conversation?.messages[0]?.content}</output>}
for(const kind of ['approval','observer'])for(const version of ['before','after'])it(`${kind} ${version}: identical 21 invalidations while first read is pending`,async()=>{
let release!:()=>void
const gate=new Promise<void>(resolve=>{release=resolve})
let active=0,peak=0
const rpc=kind==='approval'?clients.list:clients.load
rpc.mockReset().mockImplementation(async()=>{active++;peak=Math.max(peak,active);await gate;active--;return kind==='approval'?{schemaVersion:1,approvals:[]}:snapshot})
const Probe=kind==='approval'?Approval:Observer
const hook=kind==='approval'?(version==='before'?beforeApproval:afterApproval):(version==='before'?beforeObserver:afterObserver)
const screen=await render(<Probe hook={hook} seq={1}/>)
await vi.waitFor(()=>expect(rpc).toHaveBeenCalledTimes(1))
for(let seq=2;seq<=21;seq++)await screen.rerender(<Probe hook={hook} seq={seq}/>)
const blockedCalls=rpc.mock.calls.length
release()
await expect.element(screen.getByText(kind==='approval'?'settled':'same synthetic content',{exact:true})).toBeVisible()
const totalCalls=rpc.mock.calls.length
expect(totalCalls).toBe(version==='before'?21:2)
expect(peak).toBe(version==='before'?21:1)
console.log('ROUND5_REFRESH_BROWSER '+JSON.stringify({kind,version,events:21,blockedCalls,totalCalls,peakConcurrentReads:peak,terminalContentEqual:true}))
await screen.unmount()
})
