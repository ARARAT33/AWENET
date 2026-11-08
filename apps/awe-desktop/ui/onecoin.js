/* ONECOIN economy screen — wallet, exchange, contribution rewards and tiers.
   All visible strings come from the bilingual catalog (window.AWEI18N). */
(()=>{
const view=document.getElementById("view");
const T=(k,a)=>window.AWEI18N.t(k,a);
const base=()=>location.protocol==="http:"||location.protocol==="https:"?location.origin:"http://127.0.0.1:41800";
const get=async p=>{const r=await fetch(base()+p,{cache:"no-store"});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const post=async(p,b)=>{const r=await fetch(base()+p,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(b)});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const esc=x=>String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]));
const fmt=n=>(Number(n)||0).toLocaleString(undefined,{maximumFractionDigits:6})+" ONECOIN";
let offers=JSON.parse(localStorage.getItem("awenet.onecoin.offers")||"[]");
const tiers=[["FREE","tier_free_brief","tier_free_res","tier_free_reward"],["BASIC","tier_basic_brief","tier_basic_res","tier_basic_reward"],["NET+","tier_netplus_brief","tier_netplus_res","tier_netplus_reward"],["NET PRO","tier_netpro_brief","tier_netpro_res","tier_netpro_reward"],["NET ULTRA","tier_netultra_brief","tier_netultra_res","tier_netultra_reward"],["PRO","tier_pro_brief","tier_pro_res","tier_pro_reward"],["DATA GROUP","tier_datagroup_brief","tier_datagroup_res","tier_datagroup_reward"],["CENTRE GROUP","tier_centregroup_brief","tier_centregroup_res","tier_centregroup_reward"],["AWENET USER","tier_awenetuser_brief","tier_awenetuser_res","tier_awenetuser_reward"]];
function render(){
view.innerHTML="<div class=\"onecoin-hero\"><div><div class=\"eyebrow\">ONEBANK / ONECOIN</div><h1>"+T("oc_hero_title")+"</h1><p>"+T("oc_hero_sub")+"</p></div><div class=\"coin-orb\">1C</div></div>"+
"<div class=\"onecoin-grid\"><div class=\"card onecoin-stat\"><div class=\"metric-label\">"+T("oc_wallet")+"</div><div id=\"ocBalance\" class=\"metric\">"+T("oc_connecting")+"</div><div id=\"ocOwner\" class=\"muted\">"+T("oc_loading_aweid")+"</div></div><div class=\"card onecoin-stat\"><div class=\"metric-label\">"+T("oc_tier")+"</div><div id=\"ocTier\" class=\"metric\">"+T("oc_loading")+"</div><div id=\"ocContribution\" class=\"muted\">"+T("oc_contribution_verified")+"</div></div><div class=\"card onecoin-stat\"><div class=\"metric-label\">"+T("oc_fee")+"</div><div id=\"ocFee\" class=\"metric\">1%</div><div class=\"muted\">"+T("oc_fee_config")+"</div></div><div class=\"card onecoin-stat\"><div class=\"metric-label\">"+T("oc_exchange")+"</div><div class=\"metric\">P2P</div><div class=\"muted\">"+T("oc_fiat_rail")+"</div></div><div class=\"card onecoin-stat\"><div class=\"metric-label\">"+T("oc_official_price")+"</div><div id=\"ocPrice\" class=\"metric\">…</div><div class=\"muted\">"+T("oc_price_reference")+"</div></div></div>"+
"<div class=\"onecoin-tabs\"><button class=\"oc-tab active\" data-tab=\"wallet\">"+T("oc_wallet")+"</button><button class=\"oc-tab\" data-tab=\"exchange\">"+T("oc_tab_exchange")+"</button><button class=\"oc-tab\" data-tab=\"rewards\">"+T("oc_tab_rewards")+"</button><button class=\"oc-tab\" data-tab=\"tiers\">"+T("oc_tab_tiers")+"</button></div>"+
"<section id=\"oc-wallet\" class=\"oc-panel\"><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("oc_wallet")+"</h2></div><div class=\"peer-form\"><input id=\"ocRecipient\" placeholder=\""+T("oc_recipient_ph")+"\"><input id=\"ocAmount\" type=\"number\" min=\"0.000000001\" step=\"0.000000001\" placeholder=\""+T("oc_amount_ph")+"\"><input id=\"ocMemo\" placeholder=\""+T("oc_memo_ph")+"\"><button class=\"primary\" id=\"ocSend\">"+T("oc_send")+"</button><button id=\"ocReceiveQr\">"+T("oc_receive")+"</button><button id=\"ocScanQr\">"+T("oc_scan")+"</button></div><div id=\"ocWalletState\" class=\"notice\">"+T("oc_wallet_notice")+"</div></div></section>"+
"<section id=\"oc-exchange\" class=\"oc-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("oc_create_offer")+"</h2></div><div class=\"peer-form\"><select id=\"ocSide\"><option value=\"sell\">"+T("oc_sell_onecoin")+"</option><option value=\"buy\">"+T("oc_buy_onecoin")+"</option></select><input id=\"ocOfferAmount\" type=\"number\" min=\"0.000000001\" step=\"0.000000001\" placeholder=\""+T("oc_amount_ph")+"\"><input id=\"ocPrice\" type=\"number\" min=\"0.01\" step=\"0.01\" placeholder=\""+T("oc_fiat_price_ph")+"\"><select id=\"ocCurrency\"><option>USD</option><option>EUR</option><option>AMD</option><option>GBP</option></select><select id=\"ocRail\"><option>ExternalPayment</option><option>BankTransfer</option><option>Cash</option></select><button class=\"primary\" id=\"ocOffer\">"+T("oc_publish_offer")+"</button></div><div id=\"ocExchangeState\" class=\"notice\">"+T("oc_exchange_notice")+"</div></div><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("oc_offers")+"</h2></div><div id=\"ocOffers\"></div></div></section>"+
"<section id=\"oc-rewards\" class=\"oc-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("oc_rewards_title")+"</h2></div><div class=\"notice\">"+T("oc_rewards_notice")+"</div><div class=\"onecoin-form-grid\"><input id=\"ocStorage\" type=\"number\" placeholder=\""+T("oc_storage_ph")+"\"><input id=\"ocCpu\" type=\"number\" placeholder=\""+T("oc_cpu_ph")+"\"><input id=\"ocRam\" type=\"number\" placeholder=\""+T("oc_ram_ph")+"\"><input id=\"ocGpu\" type=\"number\" placeholder=\""+T("oc_gpu_ph")+"\"><input id=\"ocBandwidth\" type=\"number\" placeholder=\""+T("oc_bw_ph")+"\"><input id=\"ocUsage\" type=\"number\" min=\"0\" max=\"100\" placeholder=\""+T("oc_usage_ph")+"\"></div><button class=\"primary\" id=\"ocCalc\">"+T("oc_calculate")+"</button><div id=\"ocCalcState\" class=\"notice\">"+T("oc_calc_state")+"</div></div></section>"+
"<section id=\"oc-tiers\" class=\"oc-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("oc_tiers_title")+"</h2></div><div class=\"tier-list\">"+tiers.map(t=>"<div class=\"tier-row\"><div><b>"+esc(t[0])+"</b><small>"+T(t[1])+"</small></div><span>"+T(t[2])+"</span><strong>"+T(t[3])+"</strong></div>").join("")+"</div></div></section>";
bind();loadWallet();loadOffers();renderOffers();loadOfficialPrice();
}
function bind(){
document.querySelectorAll(".oc-tab").forEach(b=>b.onclick=()=>{document.querySelectorAll(".oc-tab").forEach(x=>x.classList.remove("active"));b.classList.add("active");document.querySelectorAll(".oc-panel").forEach(x=>x.hidden=true);document.getElementById("oc-"+b.dataset.tab).hidden=false});
const receiveBtn=document.getElementById("ocReceiveQr");
if(receiveBtn)receiveBtn.onclick=()=>showReceiveQr();
const scanBtn=document.getElementById("ocScanQr");
if(scanBtn)scanBtn.onclick=()=>scanToPay();
document.getElementById("ocSend").onclick=async()=>{const s=document.getElementById("ocWalletState"),recipient=document.getElementById("ocRecipient").value.trim(),amount=Number(document.getElementById("ocAmount").value);if(!recipient||!Number.isFinite(amount)||amount<=0){s.textContent=T("oc_send_required");return}try{const r=await post("/api/onebank/wallet/send",{recipient,amount_coins:amount,memo:document.getElementById("ocMemo").value.trim()});s.textContent=T("oc_send_ok",{id:(r.tx_id||"ok")});loadWallet()}catch(e){s.textContent=T("oc_send_fail",{message:e.message})}};
document.getElementById("ocOffer").onclick=async()=>{const s=document.getElementById("ocExchangeState"),amount=Number(document.getElementById("ocOfferAmount").value),price=Number(document.getElementById("ocPrice").value);if(amount<=0||price<=0){s.textContent=T("oc_offer_positive");return}const o={id:crypto.randomUUID(),side:document.getElementById("ocSide").value,amount,price,currency:document.getElementById("ocCurrency").value,rail:document.getElementById("ocRail").value,created_at:Date.now()};try{const r=await post("/api/onebank/exchange/offers",{offer:o});s.textContent=T("oc_offer_ok",{id:(r.offer_id||o.id)})}catch(_){offers.unshift(o);localStorage.setItem("awenet.onecoin.offers",JSON.stringify(offers));s.textContent=T("oc_offer_offline")}renderOffers()};
document.getElementById("ocCalc").onclick=async()=>{const storage=(Number(document.getElementById("ocStorage").value)||0)*1073741824,cpu=Number(document.getElementById("ocCpu").value)||0,ram=(Number(document.getElementById("ocRam").value)||0)*1073741824,gpu=Number(document.getElementById("ocGpu").value)||0,bw=(Number(document.getElementById("ocBandwidth").value)||0)*1099511627776,usage=Math.max(0,Math.min(100,Number(document.getElementById("ocUsage").value)||0));let tier="FREE";if(storage||cpu||ram||gpu||bw)tier="BASIC";if(storage>=100*107374182400&&cpu>=4&&ram>=8*1073741824&&bw>=1099511627776)tier="NET+";if(storage>=1099511627776&&cpu>=8&&ram>=16*1073741824&&gpu>=1)tier="NET PRO";const min=tier==="BASIC"?10:tier==="NET+"?45:0,score=storage/1048576+(ram/1048576)*4+cpu*1024+gpu*16384+bw/1048576,estimate=min+score*.001*(usage/100);document.getElementById("ocCalcState").textContent=T("oc_calc_result",{tier:tier,amount:estimate.toFixed(4)});try{await post("/api/onebank/contribution",{storage_bytes:Math.floor(storage),cpu_cores:cpu,ram_bytes:Math.floor(ram),gpu_units:gpu,bandwidth_bytes:Math.floor(bw),online_hours:24,node_count:(storage||cpu||ram||gpu||bw)?1:0,server_count:(storage||cpu||ram||gpu||bw)?1:0,uptime_bps:10000,utilization_bps:Math.round(usage*100)});loadWallet()}catch(_){}};
}
async function loadWallet(){try{const d=await get("/api/onebank/wallet");document.getElementById("ocBalance").textContent=fmt(d.balance_coins||0);document.getElementById("ocOwner").textContent=d.awe_id||T("oc_aweid_unavailable");document.getElementById("ocTier").textContent=d.tier||"FREE";document.getElementById("ocContribution").textContent=d.resource_score!=null?T("oc_score",{score:d.resource_score}):T("oc_contribution_unavailable");document.getElementById("ocFee").textContent=((d.fee_bps||100)/100)+"%"}catch(_){document.getElementById("ocBalance").textContent=T("oc_ledger_offline");document.getElementById("ocOwner").textContent=T("oc_start_node");document.getElementById("ocTier").textContent="FREE"}}
async function loadOfficialPrice(){try{const d=await get("/api/onecoin/price");const el=document.getElementById("ocPrice");if(el)el.textContent="$"+((d.official_price_usd_cents||0)/100).toFixed(2)}catch(_){const el=document.getElementById("ocPrice");if(el)el.textContent=T("oc_price_unavailable")}}
function showReceiveQr(){
get("/api/onebank/wallet").then(d=>{
const id=d.awe_id||"";
if(!id){return}
const qr=qrcode(0,"M");qr.addData(id);qr.make();
const box=document.createElement("div");box.className="qr-modal";
box.innerHTML="<div class=\"qr-dialog\"><h3>"+T("oc_receive_title")+"</h3><div class=\"qr-box\">"+qr.createSvgTag({cellSize:4,margin:2,scalable:true})+"</div><p class=\"muted\" style=\"word-break:break-all\">"+esc(id)+"</p><p class=\"muted\">"+T("oc_receive_hint")+"</p><button class=\"primary\" id=\"qrClose\">"+T("oc_close")+"</button></div>";
document.body.appendChild(box);
box.querySelector("#qrClose").onclick=()=>box.remove();
}).catch(()=>{const s=document.getElementById("ocWalletState");if(s)s.textContent=T("oc_wallet_unavailable");});
}
async function scanToPay(){
const s=document.getElementById('ocWalletState');if(!s)return;
if(!('BarcodeDetector' in window)||!navigator.mediaDevices||!navigator.mediaDevices.getUserMedia){s.textContent=T("oc_scan_unsupported");return}
let detector;try{detector=new BarcodeDetector({formats:['qr_code']})}catch(e){s.textContent=T("oc_scan_unsupported");return}
let stream=null,stopped=false;
const box=document.createElement('div');box.className='qr-modal';
box.innerHTML='<div class="qr-dialog"><h3>'+T("oc_scan_title")+'</h3><video autoplay playsinline style="width:100%;max-width:320px;border-radius:8px;background:#000"></video><p class="muted">'+T("oc_scan_hint")+'</p><button class="primary" id="qrScanClose">'+T("oc_cancel")+'</button></div>';
document.body.appendChild(box);
const video=box.querySelector('video');
const close=()=>{stopped=true;if(stream)stream.getTracks().forEach(t=>t.stop());box.remove()};
box.querySelector('#qrScanClose').onclick=close;
const finish=id=>{const input=document.getElementById('ocRecipient');if(input)input.value=id;close();s.textContent=T("oc_scan_filled")};
try{
stream=await navigator.mediaDevices.getUserMedia({video:{facingMode:'environment'}});
video.srcObject=stream;
const deadline=Date.now()+30000;
while(!stopped&&Date.now()<deadline){
const codes=await detector.detect(video);
for(const code of codes){
const text=String(code.rawValue||'').trim();
if(/^[0-9a-fA-F]{64}$/.test(text)){finish(text.toLowerCase());return}
}
await new Promise(r=>setTimeout(r,400));
}
if(!stopped){s.textContent=T("oc_scan_timeout");close()}
}catch(e){s.textContent=T("oc_scan_fail",{message:e.message});close()}
}

async function loadOffers(){try{const remote=await get("/api/onebank/exchange/offers");if(Array.isArray(remote)){offers=remote;localStorage.setItem("awenet.onecoin.offers",JSON.stringify(remote));renderOffers()}}catch(_){}}
function renderOffers(){const b=document.getElementById("ocOffers");if(!b)return;b.innerHTML=offers.length?offers.map(o=>"<div class=\"offer-row\"><b>"+esc(o.side.toUpperCase())+"</b><span>"+esc(o.amount)+" ONECOIN</span><span>"+esc(o.price)+" "+esc(o.currency)+"/coin</span><span>"+esc(o.rail)+"</span><small>"+T("oc_local_draft")+"</small></div>").join(""):"<div class=\"empty\">"+T("oc_no_offers")+"</div>"}
document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="onecoin"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent=T("oc_page_title");window.AWEI18N.ready().then(render)}}));
})();
