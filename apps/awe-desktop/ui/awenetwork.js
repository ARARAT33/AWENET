/* AWENETwork — official task board, submissions and participant profile.
   The backend enforces founder-only publishing; this UI is the participant
   surface: browse, accept, submit, resubmit and suggest tasks. All visible
   strings come from the bilingual catalog (window.AWEI18N). */
(()=>{
const view=document.getElementById("view");
const T=(k,a)=>window.AWEI18N.t(k,a);
const base=()=>location.protocol==="http:"||location.protocol==="https:"?location.origin:"http://127.0.0.1:41800";
const get=async p=>{const r=await fetch(base()+p,{cache:"no-store"});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const post=async(p,b)=>{const r=await fetch(base()+p,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(b)});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const esc=x=>String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]));
const coins=n=>(Number(n)||0).toLocaleString(undefined,{maximumFractionDigits:6})+" ONECOIN";
const categories=["engineering","design","content","network","community"];
const complexities=["beginner","intermediate","advanced","expert"];
const LEVEL_KEYS={"Beginner":"level_beginner","Contributor":"level_contributor","Advanced Contributor":"level_advanced","Expert":"level_expert","Master Contributor":"level_master"};
const levelText=l=>LEVEL_KEYS[l]?T(LEVEL_KEYS[l]):String(l);
let filters={category:"",complexity:"",status:"published",min:"",reload:0};

function render(){
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">AWENETwork</div><h1>"+T("awk_hero_title")+"</h1><p>"+T("awk_hero_sub")+"</p></div><div class=\"eco-orb\">⚒</div></div>"+
"<div class=\"eco-tabs\"><button class=\"eco-tab active\" data-tab=\"tasks\">"+T("awk_tab_tasks")+"</button><button class=\"eco-tab\" data-tab=\"mywork\">"+T("awk_tab_mywork")+"</button><button class=\"eco-tab\" data-tab=\"suggest\">"+T("awk_tab_suggest")+"</button></div>"+
"<section id=\"awk-tasks\" class=\"eco-panel\"><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("awk_board_title")+"</h2></div><div class=\"eco-filter\"><select id=\"awkCat\"><option value=\"\">"+T("awk_filter_all_categories")+"</option>"+categories.map(c=>"<option value=\""+c+"\">"+T("cat_"+c)+"</option>").join("")+"</select><select id=\"awkComplexity\"><option value=\"\">"+T("awk_filter_all_levels")+"</option>"+complexities.map(c=>"<option value=\""+c+"\">"+T("cx_"+c)+"</option>").join("")+"</select><select id=\"awkStatus\"><option value=\"published\">"+T("status_published")+"</option><option value=\"closed\">"+T("status_closed")+"</option><option value=\"cancelled\">"+T("status_cancelled")+"</option><option value=\"\">"+T("awk_filter_any_status")+"</option></select><input id=\"awkMin\" type=\"number\" min=\"0\" step=\"0.00000001\" placeholder=\""+T("awk_filter_min_reward")+"\"><button class=\"primary\" id=\"awkApply\">"+T("awk_apply")+"</button></div><div id=\"awkList\" class=\"task-list\"><div class=\"empty\">"+T("awk_loading_tasks")+"</div></div></div></section>"+
"<section id=\"awk-mywork\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("awk_mywork_title")+"</h2></div><div id=\"awkProfile\"><div class=\"empty\">"+T("awk_loading_profile")+"</div></div></div></section>"+
"<section id=\"awk-suggest\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("awk_tab_suggest")+"</h2></div><div class=\"notice\">"+T("awk_suggest_notice")+"</div><div class=\"eco-form-grid\"><input id=\"awkSTitle\" placeholder=\""+T("awk_title_ph")+"\"><select id=\"awkSCat\">"+categories.map(c=>"<option value=\""+c+"\">"+T("cat_"+c)+"</option>").join("")+"</select><select id=\"awkSComplexity\">"+complexities.map(c=>"<option value=\""+c+"\">"+T("cx_"+c)+"</option>").join("")+"</select><input id=\"awkSReward\" type=\"number\" min=\"0.000000001\" step=\"0.000000001\" placeholder=\""+T("awk_suggest_reward_ph")+"\"><input id=\"awkSDeadline\" type=\"number\" min=\"0\" placeholder=\""+T("awk_deadline_ph")+"\"></div><textarea id=\"awkSDescription\" rows=\"4\" placeholder=\""+T("awk_desc_ph")+"\"></textarea><textarea id=\"awkSOutcome\" rows=\"2\" placeholder=\""+T("awk_outcome_ph")+"\"></textarea><textarea id=\"awkSCriteria\" rows=\"3\" placeholder=\""+T("awk_criteria_ph")+"\"></textarea><button class=\"primary\" id=\"awkSuggestBtn\">"+T("awk_suggest_submit")+"</button><div id=\"awkSState\" class=\"notice\">"+T("awk_suggest_state")+"</div></div></section>";
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
box.innerHTML=(d.tasks||[]).length?(d.tasks||[]).map(t=>"<div class=\"task-card\"><div class=\"task-head\"><b>"+esc(t.title)+"</b><span class=\"badge badge-"+esc(t.status)+"\">"+T("status_"+t.status)+"</span></div><div class=\"task-meta\"><span>"+T("cat_"+t.category)+"</span><span>"+T("cx_"+t.complexity)+"</span><span>"+coins(t.reward_coins)+"</span>"+(t.deadline_unix?"<span>"+T("awk_deadline_short",{date:new Date(Number(t.deadline_unix)*1000).toLocaleDateString()})+"</span>":"")+"<span>"+T("awk_contributors",{count:t.contributors||0})+"</span></div><p>"+esc(String(t.description||"").slice(0,220))+(String(t.description||"").length>220?"…":"")+"</p><button class=\"primary awk-open\" data-id=\""+t.id+"\">"+T("awk_open_task")+"</button></div>").join(""):"<div class=\"empty\">"+T("awk_empty_board")+"</div>";
box.querySelectorAll(".awk-open").forEach(b=>b.onclick=()=>openTask(Number(b.dataset.id)));
}catch(e){box.innerHTML="<div class=\"empty\">"+T("awk_board_unavailable",{message:esc(e.message)})+"</div>";}
}

async function openTask(id){
try{
const d=await get("/api/awenetwork/task/"+id);
const t=d.task;
const phases=(t.phases||[]).map((p,i)=>"<div class=\"task-phase\"><b>"+T("awk_phase",{n:i+1,name:esc(p.name)})+"</b><small>"+esc(p.acceptance_criteria)+"</small><span>"+coins(p.reward_coins)+"</span></div>").join("");
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">"+T("awk_task_eyebrow",{id:t.id})+"</div><h1>"+esc(t.title)+"</h1><p>"+esc(t.description)+"</p></div><div class=\"eco-orb\">⚒</div></div>"+
"<div class=\"eco-two-col\"><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("awk_details")+"</h2></div><div class=\"kv-grid\"><span>"+T("awk_category")+"</span><b>"+T("cat_"+t.category)+"</b><span>"+T("awk_complexity")+"</span><b>"+T("cx_"+t.complexity)+"</b><span>"+T("awk_status")+"</span><b>"+T("status_"+t.status)+"</b><span>"+T("awk_reward")+"</span><b>"+coins(t.reward_coins)+"</b>"+(t.deadline_unix?"<span>"+T("awk_deadline_label")+"</span><b>"+new Date(Number(t.deadline_unix)*1000).toLocaleString()+"</b>":"")+"<span>"+T("awk_team")+"</span><b>"+(t.team_allowed?T("awk_team_allowed"):T("awk_team_solo"))+"</b><span>"+T("awk_contributors_label")+"</span><b>"+(t.contributors||0)+"</b></div><h3>"+T("awk_outcome")+"</h3><p>"+esc(t.expected_outcome)+"</p><h3>"+T("awk_criteria")+"</h3><ul>"+(t.acceptance_criteria||[]).map(c=>"<li>"+esc(c)+"</li>").join("")+"</ul>"+(phases?"<h3>"+T("awk_phases")+"</h3>"+phases:"")+"</div>"+
"<div class=\"section panel\"><div class=\"section-head\"><h2>"+T("awk_participate")+"</h2></div><div class=\"notice\">"+T("awk_participate_notice")+"</div><button class=\"primary\" id=\"awkAccept\">"+T("awk_accept")+"</button><div id=\"awkAcceptState\" class=\"notice\"></div><hr><h3>"+T("awk_submit_work")+"</h3><textarea id=\"awkEvidence\" rows=\"4\" placeholder=\""+T("awk_evidence_ph")+"\"></textarea><input id=\"awkNote\" placeholder=\""+T("awk_note_ph")+"\"><button class=\"primary\" id=\"awkSubmit\">"+T("awk_submit")+"</button><div id=\"awkSubmitState\" class=\"notice\"></div></div></div>"+
"<button class=\"eco-back\" id=\"awkBack\">"+T("awk_back")+"</button>";
document.getElementById("awkBack").onclick=()=>{render();};
document.getElementById("awkAccept").onclick=async()=>{const s=document.getElementById("awkAcceptState");try{const r=await post("/api/awenetwork/task/"+id+"/accept",{});s.textContent=T("awk_accept_ok");loadTasks()}catch(e){s.textContent=T("awk_accept_fail",{message:e.message})}};
document.getElementById("awkSubmit").onclick=async()=>{const s=document.getElementById("awkSubmitState");const evidence=String(document.getElementById("awkEvidence").value||"").split("\n").map(x=>x.trim()).filter(Boolean);if(!evidence.length){s.textContent=T("awk_evidence_required");return}try{const r=await post("/api/awenetwork/task/"+id+"/submit",{evidence,note:String(document.getElementById("awkNote").value||"").trim()||null});s.textContent=T("awk_submit_ok",{id:r.submission_id})}catch(e){s.textContent=T("awk_submit_fail",{message:e.message})}};
}catch(e){view.innerHTML="<div class=\"section panel\"><div class=\"notice\">"+T("awk_task_unavailable",{message:esc(e.message)})+"</div></div>";}
}

async function loadProfile(){
const box=document.getElementById("awkProfile");
if(!box)return;
try{
const d=await get("/api/awenetwork/profile");
const subs=(d.submissions||[]).map(s=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+s.id+" · "+esc(s.task_title||T("awk_task_ref",{id:s.task_id}))+"</b><span class=\"badge badge-"+esc(s.status)+"\">"+T("status_"+s.status)+"</span></div><div class=\"task-meta\"><span>"+T("awk_task_ref",{id:s.task_id})+"</span><span>"+new Date(Number(s.submitted_at_unix)*1000).toLocaleString()+"</span>"+(s.paid_atoms>0?"<span>"+T("awk_paid",{amount:coins(s.paid_coins)})+"</span>":"")+"</div>"+(s.review?"<p><b>"+T("awk_founder_review")+"</b> "+esc(s.review.reason)+"</p>":"")+"<div class=\"eco-evidence\">"+(s.evidence||[]).map(x=>"<small>"+esc(x)+"</small>").join("")+"</div>"+(s.status==="revising"?"<textarea class=\"awk-resub-evidence\" data-id=\""+s.id+"\" rows=\"3\" placeholder=\""+T("awk_resub_ph")+"\"></textarea><button class=\"primary awk-resubmit\" data-id=\""+s.id+"\">"+T("awk_resubmit")+"</button>":"")+"</div>").join("");
box.innerHTML="<div class=\"kv-grid\"><span>"+T("awk_aweid")+"</span><b style=\"word-break:break-all\">"+esc(d.awe_id)+"</b><span>"+T("awk_level")+"</span><b>"+esc(levelText(d.level))+"</b><span>"+T("awk_accepted_tasks")+"</span><b>"+(d.accepted_tasks||[]).length+"</b><span>"+T("awk_approved")+"</span><b>"+(d.approved||[]).length+"</b><span>"+T("awk_rejected")+"</span><b>"+(d.rejected||[]).length+"</b><span>"+T("awk_revising")+"</span><b>"+(d.revising||[]).length+"</b><span>"+T("awk_earned")+"</span><b>"+coins(d.earned&&d.earned.coins)+"</b></div><h3>"+T("awk_my_submissions")+"</h3>"+(subs||"<div class=\"empty\">"+T("awk_empty_submissions")+"</div>");
box.querySelectorAll(".awk-resubmit").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const area=box.querySelector(".awk-resub-evidence[data-id=\""+id+"\"]");const evidence=String(area.value||"").split("\n").map(x=>x.trim()).filter(Boolean);if(!evidence.length){area.placeholder=T("awk_evidence_required");return}try{await post("/api/awenetwork/submission/"+id+"/resubmit",{evidence});loadProfile()}catch(e){alert(T("awk_resubmit_fail",{message:e.message}))}});
}catch(e){box.innerHTML="<div class=\"empty\">"+T("awk_profile_unavailable",{message:esc(e.message)})+"</div>";}
}

async function submitSuggestion(){
const s=document.getElementById("awkSState");
const body={title:document.getElementById("awkSTitle").value.trim(),description:document.getElementById("awkSDescription").value.trim(),category:document.getElementById("awkSCat").value,complexity:document.getElementById("awkSComplexity").value,expected_outcome:document.getElementById("awkSOutcome").value.trim(),acceptance_criteria:String(document.getElementById("awkSCriteria").value||"").split("\n").map(x=>x.trim()).filter(Boolean),team_allowed:false};
const rewardRaw=document.getElementById("awkSReward").value.trim();
if(rewardRaw)body.reward_coins=rewardRaw;
const deadline=Number(document.getElementById("awkSDeadline").value);
if(deadline>0)body.deadline_unix=deadline;
try{const r=await post("/api/awenetwork/suggest",body);s.textContent=T("awk_suggest_ok",{id:r.suggestion_id})}catch(e){s.textContent=T("awk_suggest_fail",{message:e.message})}
}

document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="awenetwork"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent=T("awk_page_title");window.AWEI18N.ready().then(render)}}));
})();
