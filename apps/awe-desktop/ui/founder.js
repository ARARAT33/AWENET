/* Founder Control Center — the founder-only surface of the AWENET economy.
   The backend re-checks the founder key on every call; this UI simply
   exposes publish, review, price, suggestion and dispute tools. All visible
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

async function checkFounder(){
try{const d=await get("/api/founder/identity");if(d&&d.founder){const nav=document.getElementById("navFounder");if(nav)nav.hidden=false;return true}}catch(_){}
return false;
}

function render(){
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">FOUNDER CONTROL CENTER</div><h1>"+T("fnd_hero_title")+"</h1><p>"+T("fnd_hero_sub")+"</p></div><div class=\"eco-orb\">★</div></div>"+
"<div class=\"eco-tabs\"><button class=\"eco-tab active\" data-tab=\"overview\">"+T("fnd_tab_overview")+"</button><button class=\"eco-tab\" data-tab=\"publish\">"+T("fnd_tab_publish")+"</button><button class=\"eco-tab\" data-tab=\"review\">"+T("fnd_tab_review")+"</button><button class=\"eco-tab\" data-tab=\"price\">"+T("fnd_tab_price")+"</button><button class=\"eco-tab\" data-tab=\"suggestions\">"+T("fnd_tab_suggestions")+"</button><button class=\"eco-tab\" data-tab=\"store\">"+T("fnd_tab_store")+"</button><button class=\"eco-tab\" data-tab=\"passkey\">"+T("fnd_tab_passkey")+"</button></div>"+
"<section id=\"fnd-overview\" class=\"eco-panel\"><div id=\"fndOverview\"><div class=\"empty\">"+T("fnd_loading_overview")+"</div></div></section>"+
"<section id=\"fnd-publish\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("fnd_publish_title")+"</h2></div><div class=\"notice\">"+T("fnd_publish_notice")+"</div><div class=\"eco-form-grid\"><input id=\"fndTitle\" placeholder=\""+T("awk_title_ph")+"\"><select id=\"fndCat\">"+categories.map(c=>"<option value=\""+c+"\">"+T("cat_"+c)+"</option>").join("")+"</select><select id=\"fndComplexity\">"+complexities.map(c=>"<option value=\""+c+"\">"+T("cx_"+c)+"</option>").join("")+"</select><input id=\"fndReward\" type=\"text\" placeholder=\""+T("fnd_reward_ph")+"\"><input id=\"fndDeadline\" type=\"number\" min=\"0\" placeholder=\""+T("awk_deadline_ph")+"\"><label class=\"eco-check\"><input type=\"checkbox\" id=\"fndTeam\"> "+T("fnd_team_check")+"</label></div><textarea id=\"fndDescription\" rows=\"4\" placeholder=\""+T("awk_desc_ph")+"\"></textarea><textarea id=\"fndOutcome\" rows=\"2\" placeholder=\""+T("awk_outcome_ph")+"\"></textarea><textarea id=\"fndCriteria\" rows=\"3\" placeholder=\""+T("awk_criteria_ph")+"\"></textarea><textarea id=\"fndSkills\" rows=\"2\" placeholder=\""+T("fnd_skills_ph")+"\"></textarea><textarea id=\"fndPhases\" rows=\"3\" placeholder=\""+T("fnd_phases_ph")+"\"></textarea><button class=\"primary\" id=\"fndPublish\">"+T("fnd_publish_btn")+"</button><div id=\"fndPublishState\" class=\"notice\"></div></div></section>"+
"<section id=\"fnd-review\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("fnd_review_title")+"</h2></div><div class=\"notice\">"+T("fnd_review_notice")+"</div><div id=\"fndReview\"><div class=\"empty\">"+T("fnd_loading_review")+"</div></div></div></section>"+
"<section id=\"fnd-price\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("fnd_price_title")+"</h2></div><div class=\"notice\">"+T("fnd_price_notice")+"</div><div class=\"eco-form-grid\"><input id=\"fndPrice\" type=\"number\" min=\"0.01\" step=\"0.01\" placeholder=\""+T("fnd_price_ph")+"\"><input id=\"fndEffective\" type=\"number\" min=\"0\" placeholder=\""+T("fnd_effective_ph")+"\"></div><textarea id=\"fndPriceReason\" rows=\"2\" placeholder=\""+T("fnd_price_reason_ph")+"\"></textarea><button class=\"primary\" id=\"fndSetPrice\">"+T("fnd_set_price")+"</button><div id=\"fndPriceState\" class=\"notice\"></div><div id=\"fndPriceHistory\"></div></div></section>"+
"<section id=\"fnd-suggestions\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("fnd_sug_title")+"</h2></div><div class=\"notice\">"+T("fnd_sug_notice")+"</div><div id=\"fndSuggestions\"><div class=\"empty\">"+T("fnd_loading_sug")+"</div></div></div></section>"+
"<section id=\"fnd-store\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("fnd_store_title")+"</h2></div><div class=\"notice\">"+T("fnd_store_notice")+"</div><div id=\"fndStore\"><div class=\"empty\">"+T("fnd_loading_store")+"</div></div></div></section>"+
"<section id=\"fnd-passkey\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("fnd_passkey_title")+"</h2></div><div class=\"notice\">"+T("fnd_passkey_notice")+"</div><div id=\"fndPasskeys\"><div class=\"empty\">"+T("fnd_loading_passkey")+"</div></div><div class=\"eco-form-grid\"><input id=\"fndPasskeyLabel\" placeholder=\""+T("fnd_passkey_label_ph")+"\"></div><div class=\"eco-actions\"><button class=\"primary\" id=\"fndPasskeyEnroll\">"+T("fnd_enroll")+"</button><button id=\"fndPasskeyStepUp\">"+T("fnd_stepup")+"</button></div><div id=\"fndPasskeyState\" class=\"notice\"></div></div></section>";
bindTabs();loadOverview();loadReview();loadSuggestions();loadStore();loadPrice();loadPasskeys();
document.getElementById("fndPublish").onclick=publishTask;
document.getElementById("fndSetPrice").onclick=async()=>{const s=document.getElementById("fndPriceState");const cents=Number(document.getElementById("fndPrice").value);const reason=document.getElementById("fndPriceReason").value.trim();if(!(cents>0)||!reason){s.textContent=T("fnd_price_required");return}const body={new_price_usd_cents:Math.round(cents),reason};const eff=Number(document.getElementById("fndEffective").value);if(eff>0)body.effective_at_unix=eff;try{const r=await post("/api/founder/price",body);s.textContent=T("fnd_price_ok",{id:r.sequence});loadPrice()}catch(e){s.textContent=T("fnd_price_fail",{message:e.message})}};
document.getElementById("fndPasskeyEnroll").onclick=enrollPasskey;
document.getElementById("fndPasskeyStepUp").onclick=stepUp;
}

function bindTabs(){
document.querySelectorAll(".eco-tab").forEach(b=>b.onclick=()=>{document.querySelectorAll(".eco-tab").forEach(x=>x.classList.remove("active"));b.classList.add("active");document.querySelectorAll(".eco-panel").forEach(x=>x.hidden=true);document.getElementById("fnd-"+b.dataset.tab).hidden=false;});
}

function money(n){return n&&typeof n==="object"?coins(n.coins):coins(n);}

const hexToBytes=h=>{const o=new Uint8Array(h.length/2);for(let i=0;i<o.length;i++)o[i]=parseInt(h.substr(i*2,2),16);return o};
const b64uBytes=t=>{const s=String(t).replace(/-/g,'+').replace(/_/g,'/');const b=atob(s+'='.repeat((4-s.length%4)%4));const o=new Uint8Array(b.length);for(let i=0;i<b.length;i++)o[i]=b.charCodeAt(i);return o};
const b64u=b=>{const B=b instanceof Uint8Array?b:new Uint8Array(b);let s='';for(let i=0;i<B.length;i++)s+=String.fromCharCode(B[i]);return btoa(s).split('+').join('-').split('/').join('_').replace(/=+$/,'')};

async function loadPasskeys(){
const box=document.getElementById('fndPasskeys');if(!box)return;
const state=document.getElementById('fndPasskeyState');
try{
const d=await get('/api/founder/passkey/status');
const list=d.passkeys||[];
box.innerHTML=list.length?list.map(p=>'<div class="order-row"><b>'+esc(p.label)+'</b><span>'+T("fnd_enrolled_at",{date:new Date(Number(p.enrolled_at_unix)*1000).toLocaleString()})+'</span><small>'+esc(String(p.credential_id_hex||'').slice(0,16))+'…</small></div>').join(''):'<div class="empty">'+T("fnd_no_passkey")+'</div>';
if(state)state.textContent=d.session_active?T("fnd_mfa_active",{date:new Date(Number((d.session||{}).expires_at_unix||0)*1000).toLocaleString()}):T("fnd_mfa_none")+(d.enrolled?T("fnd_mfa_required_note"):'');
}catch(e){box.innerHTML='<div class="empty">'+T("fnd_passkey_state_fail",{message:esc(e.message)})+'</div>';}
}

async function enrollPasskey(){
const s=document.getElementById('fndPasskeyState');if(!s)return;
if(!(navigator.credentials&&navigator.credentials.create&&window.PublicKeyCredential)){s.textContent=T("fnd_webauthn_missing");return}
try{
const options=await post('/api/founder/passkey/register/options',{});
const label=(document.getElementById('fndPasskeyLabel').value||'').trim();
const att=await navigator.credentials.create({publicKey:{challenge:b64uBytes(options.challenge),rp:{id:options.rp.id,name:options.rp.name},user:{id:hexToBytes(options.user.id),name:options.user.name,displayName:options.user.display_name},pubKeyCredParams:options.pub_key_cred_params,attestation:'none',timeout:60000}});
const cdj=new Uint8Array(att.response.clientDataJSON);let cd='';for(let i=0;i<cdj.length;i++)cd+=String.fromCharCode(cdj[i]);
const r=await post('/api/founder/passkey/register',{label:label||undefined,client_data_json:cd,attestation_object:b64u(att.response.attestationObject)});
s.textContent=T("fnd_passkey_ok",{label:(r.label||'founder passkey')});
loadPasskeys();
}catch(e){s.textContent=T("fnd_enroll_fail",{message:e.message});}
}

async function stepUp(){
const s=document.getElementById('fndPasskeyState');if(!s)return;
if(!(navigator.credentials&&navigator.credentials.get&&window.PublicKeyCredential)){s.textContent=T("fnd_webauthn_missing_short");return}
try{
const options=await post('/api/founder/passkey/step-up/options',{});
const assertion=await navigator.credentials.get({publicKey:{challenge:b64uBytes(options.challenge),rpId:options.rp_id,userVerification:'preferred',timeout:60000}});
const cdj=new Uint8Array(assertion.response.clientDataJSON);let cd='';for(let i=0;i<cdj.length;i++)cd+=String.fromCharCode(cdj[i]);
const r=await post('/api/founder/passkey/step-up',{client_data_json:cd,authenticator_data:b64u(assertion.response.authenticatorData),signature:b64u(assertion.response.signature)});
s.textContent=T("fnd_stepup_ok",{date:new Date(Number(r.expires_at_unix||0)*1000).toLocaleString()});
loadPasskeys();
}catch(e){s.textContent=T("fnd_stepup_fail",{message:e.message});}
}

async function loadOverview(){
const box=document.getElementById("fndOverview");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const t=d.treasury||{},k=d.tasks||{},st=d.store||{};
box.innerHTML="<div class=\"eco-stat-grid\">"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_price")+"</div><div class=\"metric\">$"+((d.price&&d.price.official_price_usd_cents||0)/100).toFixed(2)+"</div><div class=\"muted\">"+T("fnd_overview_internal")+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_issued")+"</div><div class=\"metric\">"+esc(money(t.total_issued))+"</div><div class=\"muted\">"+T("fnd_overview_matches")+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_circulating")+"</div><div class=\"metric\">"+esc(money(t.circulating))+"</div><div class=\"muted\">"+T("fnd_overview_escrow",{amount:esc(money(t.escrow_held))})+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_task_rewards")+"</div><div class=\"metric\">"+esc(money(t.task_rewards))+"</div><div class=\"muted\">"+T("fnd_overview_resource",{amount:esc(money(t.resource_rewards))})+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_store_volume")+"</div><div class=\"metric\">"+esc(money(t.store_volume))+"</div><div class=\"muted\">"+T("fnd_overview_seller_income",{amount:esc(money(t.seller_income))})+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_inflation")+"</div><div class=\"metric\">"+esc(t.inflation_risk||"—")+"</div><div class=\"muted\">"+T("fnd_overview_treasury_entries",{count:esc(t.entries||0)})+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_tasks")+"</div><div class=\"metric\">"+esc(k.published||0)+"</div><div class=\"muted\">"+T("fnd_overview_submissions",{count:esc(k.submissions||0)})+" · "+T("fnd_overview_suggestions",{count:esc(k.suggestions||0)})+"</div></div>"+
"<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("fnd_overview_store")+"</div><div class=\"metric\">"+esc(st.products||0)+"</div><div class=\"muted\">"+T("fnd_overview_orders",{count:esc((st.orders||[]).length)})+" · "+T("fnd_overview_fee",{bps:esc(st.fee_bps||0)})+"</div></div></div>"+
"<h3>"+T("fnd_audit_log")+"</h3><div class=\"task-list\">"+(d.founder_log||[]).map(l=>"<div class=\"order-row\"><b>#"+l.sequence+" "+esc(l.action)+"</b><span>"+new Date(Number(l.at_unix)*1000).toLocaleString()+"</span><small>"+esc(l.detail||"")+"</small></div>").join("")+"</div>";
}catch(e){box.innerHTML="<div class=\"empty\">"+T("fnd_overview_unavailable",{message:esc(e.message)})+"</div>";}
}

async function loadReview(){
const box=document.getElementById("fndReview");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const subs=d.pending_submissions||[];
box.innerHTML=subs.length?subs.map(s=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+s.id+" · "+esc(s.task_title||T("awk_task_ref",{id:s.task_id}))+"</b><span class=\"badge badge-"+esc(s.status)+"\">"+T("status_"+s.status)+"</span></div><div class=\"task-meta\"><span>"+T("awk_task_ref",{id:s.task_id})+"</span><span>"+T("fnd_contributor",{id:esc(String(s.contributor||"").slice(0,16))})+"…</span><span>"+new Date(Number(s.submitted_at_unix)*1000).toLocaleString()+"</span></div>"+(s.note?"<p><b>"+T("fnd_note")+"</b> "+esc(s.note)+"</p>":"")+"<div class=\"eco-evidence\">"+(s.evidence||[]).map(x=>"<small>"+esc(x)+"</small>").join("")+"</div><div class=\"eco-form-grid\"><input class=\"fnd-reject-reason\" data-id=\""+s.id+"\" placeholder=\""+T("fnd_reason_ph")+"\"></div><div class=\"eco-actions\"><button class=\"primary fnd-approve\" data-id=\""+s.id+"\">"+T("fnd_approve_pay")+"</button><button class=\"fnd-approve-team-toggle\" data-id=\""+s.id+"\">"+T("fnd_team_payout")+"</button><button class=\"fnd-reject\" data-id=\""+s.id+"\">"+T("fnd_reject")+"</button><button class=\"fnd-revise\" data-id=\""+s.id+"\">"+T("fnd_revise")+"</button></div><textarea class=\"fnd-split\" data-id=\""+s.id+"\" rows=\"2\" placeholder=\""+T("fnd_split_ph")+"\" hidden></div>").join(""):"<div class=\"empty\">"+T("fnd_no_submissions")+"</div>";
box.querySelectorAll(".fnd-approve").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;try{const r=await post("/api/founder/submissions/"+id+"/approve",{});alert(T("fnd_approved_paid",{amount:money(r.paid)}));loadReview();loadOverview()}catch(e){alert(T("fnd_approve_fail",{message:e.message}))}});
box.querySelectorAll(".fnd-approve-team-toggle").forEach(b=>b.onclick=()=>{const area=box.querySelector(".fnd-split[data-id=\""+b.dataset.id+"\"]");if(area)area.hidden=!area.hidden;});
box.querySelectorAll(".fnd-reject").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const reason=box.querySelector(".fnd-reject-reason[data-id=\""+id+"\"]").value.trim();if(!reason){alert(T("fnd_reason_required"));return}try{await post("/api/founder/submissions/"+id+"/reject",{reason});loadReview();loadOverview()}catch(e){alert(T("fnd_reject_fail",{message:e.message}))}});
box.querySelectorAll(".fnd-revise").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const reason=box.querySelector(".fnd-reject-reason[data-id=\""+id+"\"]").value.trim();if(!reason){alert(T("fnd_reason_required"));return}try{await post("/api/founder/submissions/"+id+"/revise",{reason});loadReview();loadOverview()}catch(e){alert(T("fnd_revise_fail",{message:e.message}))}});
box.querySelectorAll(".fnd-split").forEach(area=>{area.addEventListener("change",async()=>{const id=area.dataset.id;const split=String(area.value||"").split("\n").map(x=>x.trim()).filter(Boolean).map(line=>{const parts=line.split("|");return{awe_id:(parts[0]||"").trim(),coins:(parts[1]||"").trim()}});if(!split.length)return;try{const r=await post("/api/founder/submissions/"+id+"/approve-team",{split});alert(T("fnd_team_ok",{amount:money(r.paid)}));loadReview();loadOverview()}catch(e){alert(T("fnd_team_fail",{message:e.message}))}});});
}catch(e){box.innerHTML="<div class=\"empty\">"+T("fnd_review_unavailable",{message:esc(e.message)})+"</div>";}
}

async function loadSuggestions(){
const box=document.getElementById("fndSuggestions");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const items=d.pending_suggestions||[];
box.innerHTML=items.length?items.map(s=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+s.id+" · "+esc(s.title)+"</b></div><div class=\"task-meta\"><span>"+T("fnd_by",{id:esc(String(s.contributor||"").slice(0,16))})+"…</span><span>"+new Date(Number(s.suggested_at_unix)*1000).toLocaleString()+"</span></div><button class=\"primary fnd-approve-sug\" data-id=\""+s.id+"\">"+T("fnd_approve_publish")+"</button></div>").join(""):"<div class=\"empty\">"+T("fnd_no_suggestions")+"</div>";
box.querySelectorAll(".fnd-approve-sug").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/founder/suggestions/"+b.dataset.id+"/approve",{});alert(T("fnd_published_task",{id:r.task_id}));loadSuggestions();loadOverview()}catch(e){alert(T("fnd_approve_fail",{message:e.message}))}});
}catch(e){box.innerHTML="<div class=\"empty\">"+T("fnd_suggestions_unavailable",{message:esc(e.message)})+"</div>";}
}

async function loadStore(){
const box=document.getElementById("fndStore");
if(!box)return;
try{
const d=await get("/api/founder/overview");
const orders=d.store&&d.store.orders||[];
box.innerHTML=orders.length?orders.map(o=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+o.order_id+" · "+esc(o.title)+"</b><span class=\"badge badge-"+esc(o.status)+"\">"+T("status_"+o.status)+"</span></div><div class=\"task-meta\"><span>"+T("mkt_buyer")+" "+esc(String(o.buyer||"").slice(0,10))+"…</span><span>"+T("mkt_seller")+" "+esc(String(o.seller||"").slice(0,10))+"…</span><span>"+money(o.amount)+"</span><span>"+T("mkt_fee",{amount:money(o.fee)})+"</span></div><div class=\"eco-actions\">"+(o.disputed?"<button class=\"fnd-resolve\" data-id=\""+o.order_id+"\" data-ruling=\"refund_buyer\">"+T("fnd_rule_refund")+"</button><button class=\"fnd-resolve\" data-id=\""+o.order_id+"\" data-ruling=\"release_seller\">"+T("fnd_rule_release")+"</button>":"")+(o.status==="held"?"<button class=\"fnd-refund\" data-id=\""+o.order_id+"\">"+T("fnd_refund_btn")+"</button>":"")+"</div></div>").join(""):"<div class=\"empty\">"+T("fnd_no_orders")+"</div>";
box.querySelectorAll(".fnd-resolve").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/founder/store/orders/"+b.dataset.id+"/resolve",{ruling:b.dataset.ruling});alert(T("fnd_settled",{amount:money(r.settled)}));loadStore();loadOverview()}catch(e){alert(T("fnd_resolve_fail",{message:e.message}))}});
box.querySelectorAll(".fnd-refund").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/founder/store/orders/"+b.dataset.id+"/refund",{});alert(T("fnd_refunded",{amount:money(r.refunded)}));loadStore();loadOverview()}catch(e){alert(T("fnd_refund_fail",{message:e.message}))}});
}catch(e){box.innerHTML="<div class=\"empty\">"+T("fnd_store_unavailable",{message:esc(e.message)})+"</div>";}
}

async function loadPrice(){
const box=document.getElementById("fndPriceHistory");
if(!box)return;
try{
const d=await get("/api/onecoin/price");
box.innerHTML="<h3>"+T("fnd_price_history")+"</h3><div class=\"task-list\">"+(d.history||[]).slice().reverse().map(h=>"<div class=\"order-row\"><b>#"+h.sequence+"</b><span>$"+(h.old_price_usd_cents/100).toFixed(2)+" → $"+(h.new_price_usd_cents/100).toFixed(2)+"</span><span>"+new Date(Number(h.effective_at_unix)*1000).toLocaleString()+"</span><small>"+esc(h.reason)+"</small></div>").join("")+"</div><p class=\"muted\">"+esc(d.disclaimer||"")+"</p>";
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
try{const r=await post("/api/awenetwork/tasks",body);s.textContent=T("fnd_publish_ok",{id:r.task_id})}catch(e){s.textContent=T("fnd_publish_fail",{message:e.message})}
}

document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="founder"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent=T("fnd_page_title");checkFounder().then(ok=>{if(ok){window.AWEI18N.ready().then(render)}else{window.AWEI18N.ready().then(()=>{view.innerHTML="<div class=\"section panel\"><div class=\"notice\">"+T("fnd_not_founder")+"</div></div>";});}});}}));
checkFounder();
})();
