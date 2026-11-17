// Release identity and exact-SHA CI gates shared by preparation and publication.
const branch = 'automation/forge-release';
const required = ['Rust CI', 'Forge Release Checks'];

function trustedReleasePr(pr, repo, sha) {
  return pr.merged_at != null && pr.merge_commit_sha === sha &&
    pr.base.ref === 'master' && pr.head.ref === branch &&
    pr.head.repo?.full_name === `${repo.owner}/${repo.repo}` &&
    pr.base.repo?.full_name === `${repo.owner}/${repo.repo}`;
}
function checksPassed(runs, sha) {
  return required.every(name => {
    const latest = runs.filter(run => run.name === name && run.head_sha === sha &&
      run.event === 'push' && run.head_branch === 'master').sort((a, b) => b.id - a.id)[0];
    return latest?.conclusion === 'success';
  });
}
async function releasePr(github, repo, sha) {
  const prs = await github.paginate(github.rest.repos.listPullRequestsAssociatedWithCommit, {...repo, commit_sha: sha, per_page: 100});
  for (const pr of prs) {
    if (!trustedReleasePr(pr, repo, sha)) continue;
    const files = await github.paginate(github.rest.pulls.listFiles, {...repo, pull_number: pr.number, per_page: 100});
    if (files.some(file => file.filename === 'release-plan.json' && file.status !== 'removed')) return pr;
  }
  return null;
}
async function gate({github, context, core}) {
  const repo = context.repo;
  const completion = context.eventName === 'workflow_run';
  if (!completion && context.eventName !== 'workflow_dispatch') throw new Error('Unsupported release event');
  const run = completion ? context.payload.workflow_run : null;
  if (completion && !run) throw new Error('Missing completed workflow payload');
  const sha = run?.head_sha ?? process.env.RELEASE_REVISION;
  if (!/^[0-9a-f]{40}$/.test(sha ?? '')) throw new Error('Release retry requires a full commit SHA');
  if (run && (run.head_branch !== 'master' || run.head_repository?.full_name !== `${repo.owner}/${repo.repo}`)) return;
  const pr = await releasePr(github, repo, sha);
  if (!pr) { core.info('Commit is not a merged release PR; no publication.'); return; }
  const {data} = await github.rest.actions.listWorkflowRunsForRepo({...repo, head_sha: sha, per_page: 100});
  if (!checksPassed(data.workflow_runs, sha)) { core.info('Waiting for both exact-commit CI workflows.'); return; }
  // crates.io rejects workflow_run OIDC identities. Dispatch the reviewed SHA
  // instead; the new run repeats these gates before requesting credentials.
  if (completion) {
    await github.rest.actions.createWorkflowDispatch({
      ...repo, workflow_id: 'forge-release.yml', ref: 'master', inputs: {revision: sha},
    });
    core.info(`Dispatched publication of reviewed release ${sha}.`);
    return;
  }
  core.setOutput('sha', sha);
}
module.exports = {branch, trustedReleasePr, checksPassed, releasePr, gate};
