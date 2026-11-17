const {test} = require('node:test');
const assert = require('node:assert/strict');
const {trustedReleasePr,checksPassed,releasePr,gate} = require('./forge-release.cjs');
const repo={owner:'org',repo:'forge'},sha='a'.repeat(40);
const good={number:1,merged_at:'2026-11-17',merge_commit_sha:sha,base:{ref:'master',repo:{full_name:'org/forge'}},head:{ref:'automation/forge-release',repo:{full_name:'org/forge'}}};
test('only the merged same-repo release branch at this SHA authorizes publishing',()=>{
  assert.equal(trustedReleasePr(good,repo,sha),true);
  for(const pr of [{...good,merged_at:null},{...good,merge_commit_sha:'b'.repeat(40)},{...good,head:{...good.head,ref:'feature'}},{...good,head:{...good.head,repo:{full_name:'attacker/fork'}}},{...good,base:{...good.base,ref:'develop'}}]) assert.equal(trustedReleasePr(pr,repo,sha),false);
});
const run=(name,id,conclusion='success')=>({name,id,conclusion,head_sha:sha,event:'push',head_branch:'master'});
test('both exact-SHA latest CI runs must succeed',()=>{
  const runs=[run('Rust CI',1),run('Forge Release Checks',2)];assert.equal(checksPassed(runs,sha),true);
  assert.equal(checksPassed(runs,'b'.repeat(40)),false);
  assert.equal(checksPassed(runs.slice(0,1),sha),false);
  assert.equal(checksPassed([...runs,run('Rust CI',3,'failure')],sha),false);
  assert.equal(checksPassed([...runs,run('Rust CI',3,null)],sha),false);
  assert.equal(checksPassed(runs.map(r=>({...r,event:'pull_request'})),sha),false);
});
test('release PR must actually change the plan and not remove it',async()=>{
  for(const [files,expected] of [[[],false],[[{filename:'release-plan.json',status:'removed'}],false],[[{filename:'release-plan.json',status:'modified'}],true]]) {
    const associated=()=>{},listFiles=()=>{};
    const github={rest:{repos:{listPullRequestsAssociatedWithCommit:associated},pulls:{listFiles}},paginate:async fn=>fn===associated?[good]:files};
    assert.equal(Boolean(await releasePr(github,repo,sha)),expected);
  }
});

// Exercise the complete gate with fake GitHub endpoints; no production workflow
// is dispatched and no publication credentials are requested by these tests.
function gateHarness({eventName='workflow_run', pr=good, files=[{filename:'release-plan.json',status:'modified'}], runs=[run('Rust CI',1),run('Forge Release Checks',2)], source={}}={}) {
  const associated=()=>{},listFiles=()=>{};
  const dispatches=[],outputs={};
  const github={
    rest:{
      repos:{listPullRequestsAssociatedWithCommit:associated},
      pulls:{listFiles},
      actions:{
        listWorkflowRunsForRepo:async()=>({data:{workflow_runs:runs}}),
        createWorkflowDispatch:async request=>{dispatches.push(request);},
      },
    },
    paginate:async endpoint=>endpoint===associated?(pr?[pr]:[]):files,
  };
  const context={repo,eventName,payload:{}};
  if(eventName==='workflow_run') context.payload.workflow_run={head_sha:sha,head_branch:'master',head_repository:{full_name:'org/forge'},...source};
  const core={info:()=>{},setOutput:(key,value)=>{outputs[key]=value;}};
  return {github,context,core,dispatches,outputs};
}

test('successful CI dispatches the fixed release SHA without authorizing a workflow_run upload',async()=>{
  const harness=gateHarness();
  await gate(harness);
  assert.deepEqual(harness.dispatches,[{...repo,workflow_id:'forge-release.yml',ref:'master',inputs:{revision:sha}}]);
  assert.deepEqual(harness.outputs,{});
});

test('workflow_dispatch repeats review and CI gates, then authorizes only the original SHA',async t=>{
  const previous=process.env.RELEASE_REVISION;
  t.after(()=>{if(previous===undefined) delete process.env.RELEASE_REVISION; else process.env.RELEASE_REVISION=previous;});
  process.env.RELEASE_REVISION=sha;
  const harness=gateHarness({eventName:'workflow_dispatch'});
  await gate(harness);
  assert.deepEqual(harness.outputs,{sha});
  assert.deepEqual(harness.dispatches,[]);
  for(const overrides of [{pr:null},{pr:{...good,merged_at:null}},{files:[]},{runs:[run('Rust CI',1)]},{runs:[run('Rust CI',1),run('Forge Release Checks',2,'failure')]}]) {
    const rejected=gateHarness({eventName:'workflow_dispatch',...overrides});
    await gate(rejected);
    assert.deepEqual(rejected.outputs,{});
    assert.deepEqual(rejected.dispatches,[]);
  }
});

test('untrusted sources, missing plans and unsuccessful latest checks cannot dispatch',async()=>{
  for(const overrides of [
    {source:{head_branch:'feature'}},
    {source:{head_repository:{full_name:'attacker/fork'}}},
    {pr:{...good,head:{...good.head,ref:'feature'}}},
    {pr:{...good,merge_commit_sha:'b'.repeat(40)}},
    {pr:{...good,merged_at:null}},
    {files:[{filename:'release-plan.json',status:'removed'}]},
    {runs:[run('Rust CI',1)]},
    {runs:[run('Rust CI',1),run('Forge Release Checks',2),run('Rust CI',3,null)]},
  ]) {
    const harness=gateHarness(overrides);
    await gate(harness);
    assert.deepEqual(harness.dispatches,[]);
    assert.deepEqual(harness.outputs,{});
  }
});

test('unsupported events, missing completion payloads and abbreviated SHAs fail closed',async()=>{
  await assert.rejects(gate(gateHarness({eventName:'push'})),/Unsupported release event/);
  const missing=gateHarness();
  delete missing.context.payload.workflow_run;
  await assert.rejects(gate(missing),/Missing completed workflow payload/);
  for(const head_sha of ['abc123','master','', 'a'.repeat(39),'a'.repeat(41)]) {
    const harness=gateHarness({source:{head_sha}});
    await assert.rejects(gate(harness),/full commit SHA/);
    assert.deepEqual(harness.dispatches,[]);
    assert.deepEqual(harness.outputs,{});
  }
});

test('dispatch errors stop the gate without falling back to a workflow_run upload',async()=>{
  const harness=gateHarness();
  harness.github.rest.actions.createWorkflowDispatch=async()=>{throw new Error('GitHub unavailable');};
  await assert.rejects(gate(harness),/GitHub unavailable/);
  assert.deepEqual(harness.outputs,{});
});
