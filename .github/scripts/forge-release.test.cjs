const {test} = require('node:test');
const assert = require('node:assert/strict');
const {trustedReleasePr,checksPassed,releasePr} = require('./forge-release.cjs');
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
