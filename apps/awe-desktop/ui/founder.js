/* Founder Control Center — the founder-only surface of the AWENET economy.
   The backend re-checks the founder key on every call; this UI simply
   exposes publish, review, price, suggestion and dispute tools. */
(()=>{
const view=document.getElementById("view");
const base=()=>location.protocol==="http:"||location.protocol==="https:"?location.origin:"http://127.0.0.1:41800";
const get=async p=>{const r=await fetch(base()+p,{cache:"no-store"});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const post=async(p,b)=>{const r=await fetch(base()+p,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(b)});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const esc=x=>String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]));
const coins=n=>(Number(n)||0).toLocaleString(undefined,{maximumFractionDigits:6})+" ONECOIN";
const categories=["engineering","design","content","network","community"];
const complexities=["beginner","intermediate","advanced","expert"];

async function checkFounder(){
try{const d=await get("/api/founder/identity");if(d&&d.founder){const nav=document.getElementById("navFounder");if(nav)nav.hidden=false;return true}}catch(_){}
return false;
}

function render(){
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">FOUNDER CONTROL CENTER</div><h1>Economy command</h1><p>Publish official AWENETwork tasks, review submissions personally, set the official ONECOIN price and settle store disputes. Every action is audit-logged.</p></div><div class=\"eco-orb\">★</div></div>"+
"<div class=\"eco-tabs\"><button class=\"eco-tab active\" data-tab=\"overview\">Overview</button><button class=\"eco-tab\" data-tab=\"publish\">Publish task</button><button class=\"eco-tab\" data-tab=\"review\">Review work</button><button class=\"eco-tab\" data-tab=\"price\">ONECOIN price</button><button class=\"eco-tab\" data-tab=\"suggestions\">Suggestions</button><button class=\"eco-tab\" data-tab=\"store\">Store orders</button></div>"+
"<section id=\"fnd-overview\" class=\"eco-panel\"><div id=\"fndOverview\"><div class=\"empty\">Loading economy overview…</div></div></section>"+
"<section id=\"fnd-publish\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>Publish official task</h2></div><div class=\"notice\">Only tasks published here are official. Set the reward and acceptance criteria before publishing; phase rewards must sum to the task reward when phases are used.</div><div class=\"eco-form-grid\"><input id=\"fndTitle\" placeholder=\"Title (required)\"><select id=\"fndCat\">"+categories.map(c=>"<option value=\""+c+"\">"+c+"</option>").join("")+"</select><select id=\"fndComplexity\">"+complexities.map(c=>"<option value=\""+c+"\">"+c+"</option>").join("")+"</select><input id=\"fndReward\" type=\"text\" placeholder=\"Reward (ONECOIN, decimal)\"><input id=\"fndDeadline\" type=\"number\" min=\"0\" placeholder=\"Deadline (unix seconds, optional)\"><label class=\"eco-check\"><input type=\"checkbox\" id=\"fndTeam\"> Allow team submissions</label></div><textarea id=\"fndDescription\" rows=\"4\" placeholder=\"Detailed description (required)\"></textarea><textarea id=\"fndOutcome\" rows=\"2\" placeholder=\"Expected outcome (required)\"></textarea><textarea id=\"fndCriteria\" rows=\"3\" placeholder=\"Acceptance criteria, one per line (at least one required)\"></textarea><textarea id=\"fndSkills\" rows=\"2\" placeholder=\"Required skills, comma separated (optional)\"></textarea><textarea id=\"fndPhases\" rows=\"3\" placeholder=\"Phases, one per line: name | acceptance criteria | reward ONECOIN (optional)\"></textarea><button class=\"primary\" id=\"fndPublish\">Publish task</button><div id=\"fndPublishState\" class=\"notice\"></div></div></section>"+
"<section id=\"fnd-review\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>Submitted work</h2></div><div class=\"notice\">Approve to mint the reward (idempotent, audited). Reject or request revision with a reason; the contributor can resubmit after a revision request.</div><div id=\"fndReview\"><div class=\"empty\">Loading submissions…</div></div></div></section>"+
"<section id=\"fnd-price\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>Official ONECOIN price</h2></div><div class=\"notice\">The official price is an internal policy reference only; it does not guarantee external exchange or redemption. Sellers price their own products in ONECOIN separately.</div><div class=\"eco-form-grid\"><input id=\"fndPrice\" type=\"number\" min=\"0.01\" step=\"0.01\" placeholder=\"New official price (USD cents)\"><input id=\"fndEffective\" type=\"number\" min=\"0\" placeholder=\"Effective at (unix seconds, optional = now)\"></div><textarea id=\"fndPriceReason\" rows=\"2\" placeholder=\"Public reason (required)\"></textarea><button class=\"primary\" id=\"fndSetPrice\">Set official price</button><div id=\"fndPriceState\" class=\"notice\"></div><div id=\"fndPriceHistory\"></div></div></section>"+
"<section id=\"fnd-suggestions\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>Community suggestions</h2></div><div class=\"notice\">Suggestions become official only when you approve and publish them.</div><div id=\"fndSuggestions\"><div class=\"empty\">Loading suggestions…</div></div></div></section>"+
"<section id=\"fnd-store\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>Store orders and disputes</h2></div><div class=\"notice\">Escrowed orders are released to the seller only after the buyer confirms delivery. Disputes are settled by the founder; refunds return the escrow to the buyer.</div><div id=\"fndStore\"><div class=\"empty\">Loading store orders…</div></div></div></section>";
bindTabs();loadOverview();loadReview();loadSuggestions();loadStore();loadPrice();
document.getElementById("fndPublish").onclick=publishTask;
document.getElementById("fndSetPrice").onclick=async()=>{const s=document.getElementById("fndPriceState");const cents=Number(document.getElementById("fndPrice").value);const reason=document.getElementById("fndPriceReason").value.trim();if(!(cents>0)||!reason){s.textContent="A positive price and a public reason are required.";return}const body={new_price_usd_cents:Math.round(cents),reason};const eff=Number(document.getElementById("fndEffective").value);if(eff>0)body.effective_at_unix=eff;try{const r=await post("/api/founder/price",body);s.textContent="Official price set: sequence #"+r.sequence+". Recorded and audit-logged.";loadPrice()}catch(e){s.textContent="Price change failed: "+e.message}};
}

function bindTabs(){
document.querySelectorAll(".eco-tab").forEach(b=>b.onclick=()=>{document.querySelectorAll(".eco-tab").forEach(x=>x.classList.remove("active"));b.classList.add("active");document.querySelectorAll(".eco-panel").forEach(x=>x.hidden=true);document.getElementById("fnd-"+b.dataset.tab).hidden=false;});
}

function money(n){return n&&typeof n==="object"?coins(n.coins):coins(n);}

async function loadOverview(){
const box=document.getElementById("fndOverview");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const t=d.treasury||{},k=d.tasks||{},st=d.store||{};
box.innerHTML="<div class=\"eco-stat-grid\">"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Official price</div><div class=\"metric\">$"+((d.price&&d.price.official_price_usd_cents||0)/100).toFixed(2)+"</div><div class=\"muted\">Internal reference</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Issued</div><div class=\"metric\">"+esc(money(t.total_issued))+"</div><div class=\"muted\">Matches ledger exactly</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Circulating</div><div class=\"metric\">"+esc(money(t.circulating))+"</div><div class=\"muted\">Escrow held: "+esc(money(t.escrow_held))+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Task rewards</div><div class=\"metric\">"+esc(money(t.task_rewards))+"</div><div class=\"muted\">Resource: "+esc(money(t.resource_rewards))+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Store volume</div><div class=\"metric\">"+esc(money(t.store_volume))+"</div><div class=\"muted\">Seller income: "+esc(money(t.seller_income))+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Inflation risk (30d)</div><div class=\"metric\">"+esc(t.inflation_risk||"—")+"</div><div class=\"muted\">Treasury entries: "+esc(t.entries||0)+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Tasks</div><div class=\"metric\">"+esc(k.published||0)+"</div><div class=\"muted\">Submissions: "+esc(k.submissions||0)+" · Suggestions: "+esc(k.suggestions||0)+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">Store</div><div class=\"metric\">"+esc(st.products||0)+"</div><div class=\"muted\">Orders: "+esc((st.orders||[]).length)+" · Fee: "+esc(st.fee_bps||0)+" bps</div></div></div>"+
"<h3>Founder audit log (latest first)</h3><div class=\"task-list\">"+(d.founder_log||[]).map(l=>"<div class=\"order-row\"><b>#"+l.sequence+" "+esc(l.action)+"</b><span>"+new Date(Number(l.at_unix)*1000).toLocaleString()+"</span><small>"+esc(l.detail||"")+"</small></div>").join("")+"</div>";
}catch(e){box.innerHTML="<div class=\"empty\">Overview unavailable: "+esc(e.message)+"</div>";}
}

async function loadReview(){
const box=document.getElementById("fndReview");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const subs=d.pending_submissions||[];
box.innerHTML=subs.length?subs.map(s=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+s.id+" · "+esc(s.task_title||("task "+s.task_id))+"</b><span class=\"badge badge-"+esc(s.status)+"\">"+esc(s.status)+"</span></div><div class=\"task-meta\"><span>task #"+s.task_id+"</span><span>contributor "+esc(String(s.contributor||"").slice(0,16))+"…</span><span>"+new Date(Number(s.submitted_at_unix)*1000).toLocaleString()+"</span></div>"+(s.note?"<p><b>Note:</b> "+esc(s.note)+"</p>":"")+"<div class=\"eco-evidence\">"+(s.evidence||[]).map(x=>"<small>"+esc(x)+"</small>").join("")+"</div><div class=\"eco-form-grid\"><input class=\"fnd-reject-reason\" data-id=\""+s.id+"\" placeholder=\"Reason (required for reject/revise)\"></div><div class=\"eco-actions\"><button class=\"primary fnd-approve\" data-id=\""+s.id+"\">Approve & pay</button><button class=\"fnd-approve-team-toggle\" data-id=\""+s.id+"\">Team payout</button><button class=\"fnd-reject\" data-id=\""+s.id+"\">Reject</button><button class=\"fnd-revise\" data-id=\""+s.id+"\">Request revision</button></div><textarea class=\"fnd-split\" data-id=\""+s.id+"\" rows=\"2\" placeholder=\"Team split, one per line: AWE_ID (64 hex) | ONECOIN amount\" hidden></div>").join(""):"<div class=\"empty\">No submissions awaiting review.</div>";
box.querySelectorAll(".fnd-approve").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;try{const r=await post("/api/founder/submissions/"+id+"/approve",{});alert("Approved. Paid: "+money(r.paid));loadReview();loadOverview()}catch(e){alert("Approve failed: "+e.message)}});
box.querySelectorAll(".fnd-approve-team-toggle").forEach(b=>b.onclick=()=>{const area=box.querySelector(".fnd-split[data-id=\""+b.dataset.id+"\"]");if(area)area.hidden=!area.hidden;});
box.querySelectorAll(".fnd-reject").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const reason=box.querySelector(".fnd-reject-reason[data-id=\""+id+"\"]").value.trim();if(!reason){alert("A reason is required.");return}try{await post("/api/founder/submissions/"+id+"/reject",{reason});loadReview();loadOverview()}catch(e){alert("Reject failed: "+e.message)}});
box.querySelectorAll(".fnd-revise").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const reason=box.querySelector(".fnd-reject-reason[data-id=\""+id+"\"]").value.trim();if(!reason){alert("A reason is required.");return}try{await post("/api/founder/submissions/"+id+"/revise",{reason});loadReview();loadOverview()}catch(e){alert("Revise failed: "+e.message)}});
box.querySelectorAll(".fnd-split").forEach(area=>{area.addEventListener("change",async()=>{const id=area.dataset.id;const split=String(area.value||"").split("\n").map(x=>x.trim()).filter(Boolean).map(line=>{const parts=line.split("|");return{awe_id:(parts[0]||"").trim(),coins:(parts[1]||"").trim()}});if(!split.length)return;try{const r=await post("/api/founder/submissions/"+id+"/approve-team",{split});alert("Team payout done: "+money(r.paid));loadReview();loadOverview()}catch(e){alert("Team payout failed: "+e.message)}});});
}catch(e){box.innerHTML="<div class=\"empty\">Review queue unavailable: "+esc(e.message)+"</div>";}
}

async function loadSuggestions(){
const box=document.getElementById("fndSuggestions");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const items=d.pending_suggestions||[];
box.innerHTML=items.length?items.map(s=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+s.id+" · "+esc(s.title)+"</b></div><div class=\"task-meta\"><span>by "+esc(String(s.contributor||"").slice(0,16))+"…</span><span>"+new Date(Number(s.suggested_at_unix)*1000).toLocaleString()+"</span></div><button class=\"primary fnd-approve-sug\" data-id=\""+s.id+"\">Approve & publish as official task</button></div>").join(""):"<div class=\"empty\">No pending suggestions.</div>";
box.querySelectorAll(".fnd-approve-sug").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/founder/suggestions/"+b.dataset.id+"/approve",{});alert("Published as task #"+r.task_id);loadSuggestions();loadOverview()}catch(e){alert("Approve failed: "+e.message)}});
}catch(e){box.innerHTML="<div class=\"empty\">Suggestions unavailable: "+esc(e.message)+"</div>";}
}

async function loadStore(){
const box=document.getElementById("fndStore");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const orders=d.store&&d.store.orders||[];
box.innerHTML=orders.length?orders.map(o=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+o.order_id+" · "+esc(o.title)+"</b><span class=\"badge badge-"+esc(o.status)+"\">"+esc(o.status)+"</span></div><div class=\"task-meta\"><span>buyer "+esc(String(o.buyer||"").slice(0,10))+"…</span><span>seller "+esc(String(o.seller||"").slice(0,10))+"…</span><span>"+money(o.amount)+"</span><span>fee "+money(o.fee)+"</span></div><div class=\"eco-actions\">"+(o.disputed?"<button class=\"fnd-resolve\" data-id=\""+o.order_id+"\" data-ruling=\"refund_buyer\">Rule: refund buyer</button><button class=\"fnd-resolve\" data-id=\""+o.order_id+"\" data-ruling=\"release_seller\">Rule: release to seller</button>":"")+(o.status==="held"?"<button class=\"fnd-refund\" data-id=\""+o.order_id+"\">Founder refund</button>":"")+"</div></div>").join(""):"<div class=\"empty\">No store orders yet.</div>";
box.querySelectorAll(".fnd-resolve").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/founder/store/orders/"+b.dataset.id+"/resolve",{ruling:b.dataset.ruling});alert("Dispute settled: "+money(r.settled));loadStore();loadOverview()}catch(e){alert("Resolve failed: "+e.message)}});
box.querySelectorAll(".fnd-refund").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/founder/store/orders/"+b.dataset.id+"/refund",{});alert("Refunded: "+money(r.refunded));loadStore();loadOverview()}catch(e){alert("Refund failed: "+e.message)}});
}catch(e){box.innerHTML="<div class=\"empty\">Store orders unavailable: "+esc(e.message)+"</div>";}
}

async function loadPrice(){
const box=document.getElementById("fndPriceHistory");
if(!box)return;
try{
const d=await get("/api/onecoin/price");
box.innerHTML="<h3>Price history</h3><div class=\"task-list\">"+(d.history||[]).slice().reverse().map(h=>"<div class=\"order-row\"><b>#"+h.sequence+"</b><span>$"+(h.old_price_usd_cents/100).toFixed(2)+" → $"+(h.new_price_usd_cents/100).toFixed(2)+"</span><span>"+new Date(Number(h.effective_at_unix)*1000).toLocaleString()+"</span><small>"+esc(h.reason)+"</small></div>").join("")+"</div><p class=\"muted\">"+esc(d.disclaimer||"")+"</p>";
}catch(_){box.innerHTML="";}
}

async function publishTask(){
const s=document.getElementById("fndPublishState");
const phases=String(document.getElementById("fndPhases").value||"").split("\n").map(x=>x.trim()).filter(Boolean).map(line=>{const p=line.split("|");return{name:(p[0]||"").trim(),acceptance_criteria:(p[1]||"").trim(),reward_coins:(p[2]||"").trim()}}).filter(p=>p.name);
const body={title:document.getElementById("fndTitle").value.trim(),description:document.getElementById("fndDescription").value.trim(),category:document.getElementById("fndCat").value,complexity:document.getElementById("fndComplexity").value,expected_outcome:document.getElementById("fndOutcome").value.trim(),acceptance_criteria:String(document.getElementById("fndCriteria").value||"").split("\n").map(x=>x.trim()).filter(Boolean),required_skills:String(document.getElementById("fndSkills").value||"").split(",").map(x=>x.trim()).filter(Boolean),team_allowed:document.getElementById("fndTeam").checked,submission_format:"link_or_attachment"};
const rewardRaw=document.getElementById("fndReward").value.trim();
if(rewardRaw)body.reward_coins=rewardRaw;
if(phases.length)body.phases=phases;
const deadline=Number(document.getElementById("fndDeadline").value);
if(deadline>0)body.deadline_unix=deadline;
try{const r=await post("/api/awenetwork/tasks",body);s.textContent="Published as official task #"+r.task_id+". It is now visible on the AWENETwork board."}catch(e){s.textContent="Publish failed: "+e.message}
}

document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="founder"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent="Founder Center";checkFounder().then(ok=>{if(ok){render()}else{view.innerHTML="<div class=\"section panel\"><div class=\"notice\">This AWE ID is not the AWENET founder. The Founder Control Center is available only to the founder identity installed at economy genesis.</div></div>";}});}}));
checkFounder();
})();
