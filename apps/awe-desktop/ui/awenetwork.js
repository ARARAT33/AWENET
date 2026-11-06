/* AWENETwork — official task board, submissions and participant profile.
   The backend enforces founder-only publishing; this UI is the participant
   surface: browse, accept, submit, resubmit and suggest tasks. */
(()=>{
const view=document.getElementById("view");
const base=()=>location.protocol==="http:"||location.protocol==="https:"?location.origin:"http://127.0.0.1:41800";
const get=async p=>{const r=await fetch(base()+p,{cache:"no-store"});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const post=async(p,b)=>{const r=await fetch(base()+p,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(b)});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const esc=x=>String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]));
const coins=n=>(Number(n)||0).toLocaleString(undefined,{maximumFractionDigits:6})+" ONECOIN";
const categories=["engineering","design","content","network","community"];
const complexities=["beginner","intermediate","advanced","expert"];
let filters={category:"",complexity:"",status:"published",min:"",reload:0};

function render(){
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">AWENETwork</div><h1>Official work platform</h1><p>Founder-published tasks for the AWENET network. Accept a task, deliver real work, and the founder reviews every submission before ONECOIN is minted.</p></div><div class=\"eco-orb\">⚒</div></div>"+
"<div class=\"eco-tabs\"><button class=\"eco-tab active\" data-tab=\"tasks\">Tasks</button><button class=\"eco-tab\" data-tab=\"mywork\">My Work</button><button class=\"eco-tab\" data-tab=\"suggest\">Suggest a task</button></div>"+
"<section id=\"awk-tasks\" class=\"eco-panel\"><div class=\"section panel\"><div class=\"section-head\"><h2>Task board</h2></div><div class=\"eco-filter\"><select id=\"awkCat\"><option value=\"\">All categories</option>"+categories.map(c=>"<option value=\""+c+"\">"+c+"</option>").join("")+"</select><select id=\"awkComplexity\"><option value=\"\">All levels</option>"+complexities.map(c=>"<option value=\""+c+"\">"+c+"</option>").join("")+"</select><select id=\"awkStatus\"><option value=\"published\">Published</option><option value=\"closed\">Closed</option><option value=\"cancelled\">Cancelled</option><option value=\"\">Any status</option></select><input id=\"awkMin\" type=\"number\" min=\"0\" step=\"0.00000001\" placeholder=\"Min reward (ONECOIN)\"><button class=\"primary\" id=\"awkApply\">Apply filters</button></div><div id=\"awkList\" class=\"task-list\"><div class=\"empty\">Loading tasks…</div></div></div></section>"+
"<section id=\"awk-mywork\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>My work profile</h2></div><div id=\"awkProfile\"><div class=\"empty\">Loading profile…</div></div></div></section>"+
"<section id=\"awk-suggest\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>Suggest a task</h2></div><div class=\"notice\">Suggestions are community proposals. They never become official tasks until the founder reviews and publishes them. No reward is promised before publication.</div><div class=\"eco-form-grid\"><input id=\"awkSTitle\" placeholder=\"Title (required)\"><select id=\"awkSCat\">"+categories.map(c=>"<option value=\""+c+"\">"+c+"</option>").join("")+"</select><select id=\"awkSComplexity\">"+complexities.map(c=>"<option value=\""+c+"\">"+c+"</option>").join("")+"</select><input id=\"awkSReward\" type=\"number\" min=\"0.000000001\" step=\"0.000000001\" placeholder=\"Suggested reward (ONECOIN)\"><input id=\"awkSDeadline\" type=\"number\" min=\"0\" placeholder=\"Deadline (unix seconds, optional)\"></div><textarea id=\"awkSDescription\" rows=\"4\" placeholder=\"Detailed description (required)\"></textarea><textarea id=\"awkSOutcome\" rows=\"2\" placeholder=\"Expected outcome (required)\"></textarea><textarea id=\"awkSCriteria\" rows=\"3\" placeholder=\"Acceptance criteria, one per line (at least one required)\"></textarea><button class=\"primary\" id=\"awkSuggestBtn\">Submit suggestion</button><div id=\"awkSState\" class=\"notice\">Awaiting founder approval — nothing is official until published.</div></div></section>";
bindTabs();bindFilters();loadTasks();loadProfile();
}

function bindTabs(){
document.querySelectorAll(".eco-tab").forEach(b=>b.onclick=()=>{document.querySelectorAll(".eco-tab").forEach(x=>x.classList.remove("active"));b.classList.add("active");document.querySelectorAll(".eco-panel").forEach(x=>x.hidden=true);document.getElementById("awk-"+b.dataset.tab).hidden=false;});
document.querySelectorAll(".eco-tab").forEach(b=>{if(b.dataset.tab==="mywork")b.addEventListener("click",loadProfile)});
}

function bindFilters(){
document.getElementById("awkApply").onclick=()=>{filters.category=document.getElementById("awkCat").value;filters.complexity=document.getElementById("awkComplexity").value;filters.status=document.getElementById("awkStatus").value;filters.min=document.getElementById("awkMin").value;loadTasks();};
document.getElementById("awkSuggestBtn").onclick=submitSuggestion;
}

function taskQuery(){
const q=new URLSearchParams();
if(filters.category)q.set("category",filters.category);
if(filters.complexity)q.set("complexity",filters.complexity);
if(filters.status)q.set("status",filters.status);
if(filters.min&&Number(filters.min)>0)q.set("min_reward_coins",filters.min);
return q.toString();
}

async function loadTasks(){
const box=document.getElementById("awkList");
if(!box)return;
try{
const d=await get("/api/awenetwork/tasks"+(taskQuery()?"?"+taskQuery():""));
box.innerHTML=(d.tasks||[]).length?(d.tasks||[]).map(t=>"<div class=\"task-card\"><div class=\"task-head\"><b>"+esc(t.title)+"</b><span class=\"badge badge-"+esc(t.status)+"\">"+esc(t.status)+"</span></div><div class=\"task-meta\"><span>"+esc(t.category)+"</span><span>"+esc(t.complexity)+"</span><span>"+coins(t.reward_coins)+"</span>"+(t.deadline_unix?"<span>deadline "+new Date(Number(t.deadline_unix)*1000).toLocaleDateString()+"</span>":"")+"<span>"+(t.contributors||0)+" contributor(s)</span></div><p>"+esc(String(t.description||"").slice(0,220))+(String(t.description||"").length>220?"…":"")+"</p><button class=\"primary awk-open\" data-id=\""+t.id+"\">Open task</button></div>").join(""):"<div class=\"empty\">No tasks match these filters yet. The founder publishes official tasks here.</div>";
box.querySelectorAll(".awk-open").forEach(b=>b.onclick=()=>openTask(Number(b.dataset.id)));
}catch(e){box.innerHTML="<div class=\"empty\">Task board unavailable: "+esc(e.message)+"</div>";}
}

async function openTask(id){
try{
const d=await get("/api/awenetwork/task/"+id);
const t=d.task;
const phases=(t.phases||[]).map((p,i)=>"<div class=\"task-phase\"><b>Phase "+(i+1)+": "+esc(p.name)+"</b><small>"+esc(p.acceptance_criteria)+"</small><span>"+coins(p.reward_coins)+"</span></div>").join("");
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">AWENETwork / Task #"+t.id+"</div><h1>"+esc(t.title)+"</h1><p>"+esc(t.description)+"</p></div><div class=\"eco-orb\">⚒</div></div>"+
"<div class=\"eco-two-col\"><div class=\"section panel\"><div class=\"section-head\"><h2>Details</h2></div><div class=\"kv-grid\"><span>Category</span><b>"+esc(t.category)+"</b><span>Complexity</span><b>"+esc(t.complexity)+"</b><span>Status</span><b>"+esc(t.status)+"</b><span>Reward</span><b>"+coins(t.reward_coins)+"</b>"+(t.deadline_unix?"<span>Deadline</span><b>"+new Date(Number(t.deadline_unix)*1000).toLocaleString()+"</b>":"")+"<span>Team work</span><b>"+(t.team_allowed?"allowed":"solo")+"</b><span>Contributors</span><b>"+(t.contributors||0)+"</b></div><h3>Expected outcome</h3><p>"+esc(t.expected_outcome)+"</p><h3>Acceptance criteria</h3><ul>"+(t.acceptance_criteria||[]).map(c=>"<li>"+esc(c)+"</li>").join("")+"</ul>"+(phases?"<h3>Phases</h3>"+phases:"")+"</div>"+
"<div class=\"section panel\"><div class=\"section-head\"><h2>Participate</h2></div><div class=\"notice\">Accept the task first, then submit evidence of your work. The founder personally reviews every submission; the reward is paid only after approval.</div><button class=\"primary\" id=\"awkAccept\">Accept this task</button><div id=\"awkAcceptState\" class=\"notice\"></div><hr><h3>Submit work</h3><textarea id=\"awkEvidence\" rows=\"4\" placeholder=\"Evidence: one link, file reference or proof per line (at least 1, max 16, 512 chars each)\"></textarea><input id=\"awkNote\" placeholder=\"Note for the reviewer (optional)\"><button class=\"primary\" id=\"awkSubmit\">Submit for review</button><div id=\"awkSubmitState\" class=\"notice\"></div></div></div>"+
"<button class=\"eco-back\" id=\"awkBack\">← Back to the task board</button>";
document.getElementById("awkBack").onclick=()=>{render();};
document.getElementById("awkAccept").onclick=async()=>{const s=document.getElementById("awkAcceptState");try{const r=await post("/api/awenetwork/task/"+id+"/accept",{});s.textContent="Task accepted. You can now work and submit.";loadTasks()}catch(e){s.textContent="Accept failed: "+e.message}};
document.getElementById("awkSubmit").onclick=async()=>{const s=document.getElementById("awkSubmitState");const evidence=String(document.getElementById("awkEvidence").value||"").split("\n").map(x=>x.trim()).filter(Boolean);if(!evidence.length){s.textContent="Add at least one line of evidence.";return}try{const r=await post("/api/awenetwork/task/"+id+"/submit",{evidence,note:String(document.getElementById("awkNote").value||"").trim()||null});s.textContent="Submitted (#"+r.submission_id+"). Status: awaiting founder review. No reward is paid before approval."}catch(e){s.textContent="Submit failed: "+e.message}};
}catch(e){view.innerHTML="<div class=\"section panel\"><div class=\"notice\">Task unavailable: "+esc(e.message)+"</div></div>";}
}

async function loadProfile(){
const box=document.getElementById("awkProfile");
if(!box)return;
try{
const d=await get("/api/awenetwork/profile");
const subs=(d.submissions||[]).map(s=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+s.id+" · "+esc(s.task_title||("task "+s.task_id))+"</b><span class=\"badge badge-"+esc(s.status)+"\">"+esc(s.status)+"</span></div><div class=\"task-meta\"><span>task #"+s.task_id+"</span><span>"+new Date(Number(s.submitted_at_unix)*1000).toLocaleString()+"</span>"+(s.paid_atoms>0?"<span>paid "+coins(s.paid_coins)+"</span>":"")+"</div>"+(s.review?"<p><b>Founder review:</b> "+esc(s.review.reason)+"</p>":"")+"<div class=\"eco-evidence\">"+(s.evidence||[]).map(x=>"<small>"+esc(x)+"</small>").join("")+"</div>"+(s.status==="revising"?"<textarea class=\"awk-resub-evidence\" data-id=\""+s.id+"\" rows=\"3\" placeholder=\"New evidence, one per line\"></textarea><button class=\"primary awk-resubmit\" data-id=\""+s.id+"\">Resubmit</button>":"")+"</div>").join("");
box.innerHTML="<div class=\"kv-grid\"><span>AWE ID</span><b style=\"word-break:break-all\">"+esc(d.awe_id)+"</b><span>Level</span><b>"+esc(d.level)+"</b><span>Accepted tasks</span><b>"+(d.accepted_tasks||[]).length+"</b><span>Approved</span><b>"+(d.approved||[]).length+"</b><span>Rejected</span><b>"+(d.rejected||[]).length+"</b><span>Revising</span><b>"+(d.revising||[]).length+"</b><span>Earned</span><b>"+coins(d.earned&&d.earned.coins)+"</b></div><h3>My submissions</h3>"+(subs||"<div class=\"empty\">No submissions yet. Accept a task from the board and submit your work.</div>");
box.querySelectorAll(".awk-resubmit").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const area=box.querySelector(".awk-resub-evidence[data-id=\""+id+"\"]");const evidence=String(area.value||"").split("\n").map(x=>x.trim()).filter(Boolean);if(!evidence.length){area.placeholder="Add at least one line of evidence.";return}try{await post("/api/awenetwork/submission/"+id+"/resubmit",{evidence});loadProfile()}catch(e){alert("Resubmit failed: "+e.message)}});
}catch(e){box.innerHTML="<div class=\"empty\">Profile unavailable: "+esc(e.message)+"</div>";}
}

async function submitSuggestion(){
const s=document.getElementById("awkSState");
const body={title:document.getElementById("awkSTitle").value.trim(),description:document.getElementById("awkSDescription").value.trim(),category:document.getElementById("awkSCat").value,complexity:document.getElementById("awkSComplexity").value,expected_outcome:document.getElementById("awkSOutcome").value.trim(),acceptance_criteria:String(document.getElementById("awkSCriteria").value||"").split("\n").map(x=>x.trim()).filter(Boolean),team_allowed:false};
const rewardRaw=document.getElementById("awkSReward").value.trim();
if(rewardRaw)body.reward_coins=rewardRaw;
const deadline=Number(document.getElementById("awkSDeadline").value);
if(deadline>0)body.deadline_unix=deadline;
try{const r=await post("/api/awenetwork/suggest",body);s.textContent="Suggestion #"+r.suggestion_id+" recorded. It becomes an official task only if the founder approves and publishes it."}catch(e){s.textContent="Suggestion rejected: "+e.message}
}

document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="awenetwork"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent="AWENETwork";render()}}));
})();
