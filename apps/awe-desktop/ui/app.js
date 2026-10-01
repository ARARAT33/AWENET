const view=document.getElementById("view"),title=document.getElementById("pageTitle"),navs=[...document.querySelectorAll(".nav")];
const pages={dashboard:["Overview","Network-wide status at a glance"],node:["My Node","Your identity, runtime and listening endpoint"],network:["Peers & Connections","Discover and connect to AWEp2P nodes"],storage:["Storage","Local and distributed data plane"],messenger:["Messenger","Peer-to-peer messaging"],store:["AWEStore","AWE modules and services"],security:["Security","Identity, transport and trust"],diagnostics:["Diagnostics","Health checks and runtime inspection"],settings:["Settings","Application configuration"]};
let live={status:"starting",node_id:"loading",node_address:"loading",transport:"loading",ui:"connecting",peers:[],node:{},storage:{},security:{}};

function apiBase(){return localStorage.getItem("aweApiBase")||""}
async function api(path,options={}){const r=await fetch(apiBase()+path,options);if(!r.ok)throw new Error(await r.text());return r.json()}
async function loadMessenger(){try{const d=await api("/api/messenger");const box=document.getElementById("messageList");if(box)box.innerHTML=d.messages.length?d.messages.map(m=>'<div class="list-row"><span>'+esc(m.recipient)+'</span><b>'+esc(m.state)+'</b><span>'+esc(m.text)+'</span></div>').join(""):'<div class="empty">No queued messages.</div>'}catch(e){}}\nasync function refresh(){
 try{const [status,node,storage,security]=await Promise.all([api("/api/status"),api("/api/node"),api("/api/storage"),api("/api/security")]);live={...status,node,storage,security};setConnection(true)}
 catch(e){live={...live,status:"offline",ui:"disconnected",peers:[]};setConnection(false)}
}
function setConnection(on){document.getElementById("sideDot").classList.toggle("online",on);document.getElementById("sideState").textContent=on?"Node online":"Disconnected";document.getElementById("sideTransport").textContent=on?live.transport:"API unavailable";document.getElementById("apiBadge").textContent="API · "+(on?live.ui:"offline")}
function esc(x){return String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]))}
function card(a,b,c="Live runtime"){return '<div class="card"><div class="metric-label">'+a+'</div><div class="metric">'+esc(b)+'</div><div class="muted">'+c+'</div></div>'}
function panel(h,body,extra=""){return '<div class="section panel"><div class="section-head"><h2>'+h+'</h2>'+extra+'</div>'+body+'</div>'}
function peerRows(){return live.peers.length?live.peers.map(p=>'<tr><td><div class="peer"><i class="peer-dot"></i><b>'+esc(p.id)+'</b></div></td><td>'+esc(p.address)+'</td><td>'+esc(p.last_seen)+'</td><td><span class="status"><i></i>Discovered</span></td></tr>').join(""):'<tr><td colspan="4" class="empty">No peers are currently known to this node.</td></tr>'}
function render(k){
 const p=pages[k]||pages.dashboard;navs.forEach(n=>n.classList.toggle("active",n.dataset.view===k));title.textContent=p[0];
 let body="";
 if(k==="dashboard"){
  body='<div class="grid">'+card("Node status",live.status.toUpperCase())+card("Data centre",live.node?.descriptor?"Node registered":"Local node","Live node descriptor")+card("Node ID",live.node_id,"AWE identity")+card("Connected peers",live.peers.length,"Current routing table")+card("Transport",live.transport,"Node transport")+'</div>'+
  '<div class="section two">'+panel("Network topology",'<div class="network-map"><div class="node-point main" style="left:49%;top:47%"></div>'+live.peers.slice(0,8).map((_,i)=>{const a=i*45;return '<div class="line" style="left:51%;top:51%;width:105px;transform:rotate('+a+'deg)"></div><div class="node-point" style="left:'+(50+34*Math.cos(a*Math.PI/180))+'%;top:'+(50+34*Math.sin(a*Math.PI/180))+'%"></div>'}).join("")+'</div>'),panel("Runtime health",'<div class="big-status"><div class="big-orb">'+(live.status==="online"?"✓":"!")+'</div><div><b>'+esc(live.status==="online"?"Node operational":"Node unavailable")+'</b><div class="detail">'+esc(live.node_address)+'</div></div></div><div class="list section"><div class="list-row"><span>Core API</span><span class="status"><i></i>'+esc(live.ui)+'</span></div><div class="list-row"><span>Known peers</span><b>'+live.peers.length+'</b></div></div>'))+
  panel("Known peers",'<table class="table"><thead><tr><th>Peer</th><th>Address</th><th>Last seen</th><th>State</th></tr></thead><tbody>'+peerRows()+'</tbody></table>','<button class="secondary" id="goNetwork">Manage</button>');
 } else if(k==="node"){
  body='<div class="grid">'+card("Status",live.status.toUpperCase())+card("Identity",live.node_id,"Public AWE identity")+card("Listen",live.node_address,"Local node endpoint")+card("Transport",live.transport,"Active protocol")+'</div>'+
  panel("Identity",'<div class="list"><div class="list-row"><span>AWE Node ID</span><b>'+esc(live.node_id)+'</b></div><div class="list-row"><span>Listen address</span><b>'+esc(live.node_address)+'</b></div><div class="list-row"><span>Transport</span><b>'+esc(live.transport)+'</b></div><div class="list-row"><span>Known peers</span><b>'+live.peers.length+'</b></div></div>')+
  panel("Runtime",'<div class="notice">This screen represents the actual local Rust node. Identity and endpoint values come from the running node API.</div>');
 } else if(k==="network"){
  body=panel("Connect to node",'<div class="peer-form"><input id="peerAddress" placeholder="127.0.0.1:41000"><button class="primary" id="connectBtn">Connect</button></div><div class="muted" style="margin-top:8px">Enter a reachable AWEp2P node address. The Rust node performs bootstrap/discovery.</div>')+
  panel("Known peers",'<table class="table"><thead><tr><th>Peer</th><th>Address</th><th>Last seen</th><th>State</th></tr></thead><tbody>'+peerRows()+'</tbody></table>','<button class="secondary" id="peerRefresh">Refresh</button>')+
  panel("Topology",'<div class="network-map"><div class="node-point main" style="left:49%;top:47%"></div>'+live.peers.map((_,i)=>{const a=i*(360/Math.max(live.peers.length,1));return '<div class="line" style="left:51%;top:51%;width:100px;transform:rotate('+a+'deg)"></div><div class="node-point" style="left:'+(50+35*Math.cos(a*Math.PI/180))+'%;top:'+(50+35*Math.sin(a*Math.PI/180))+'%"></div>'}).join("")+'</div>');
 } else if(k==="storage"){
  const used=localStorage.getItem("aweStoragePath")||"Node-managed storage";
  body='<div class="grid">'+card("Data plane","Active","Storage subsystem")+card("Storage path",live.storage.root||used,"Real node storage")+card("Replication","Core-managed","No invented counters")+card("Encryption",live.security.transport||"Core","Real transport")+'</div>'+
  panel("Storage status",'<div class="notice">Storage operations are owned by the Rust core. This UI does not fabricate capacity or replication statistics that the API does not expose.</div>')+
  panel("Configured location",'<div class="peer-form"><input id="storagePath" value="'+esc(localStorage.getItem("aweStoragePath")||"")+'" placeholder="Optional local storage path"><button class="primary" id="saveStorage">Save</button></div>');
 } else if(k==="messenger"){
  body=panel("Messenger",'<div class="peer-form"><input id="msgRecipient" placeholder="Recipient AWE ID"><input id="msgText" placeholder="Message"><button class="primary" id="sendMsg">Send</button></div><div id="msgState" class="muted" style="margin-top:8px">Messages are queued locally until transport delivery is available.</div>')+
  panel("Local message queue",'<div id="messageList" class="list"><div class="empty">Loading…</div></div>');
 } else if(k==="store"){
  body='<div class="store-grid"><div class="card store-card"><div class="store-icon">◈</div><b>AWE Core</b><p>Core networking, identity, routing and node services.</p><span class="status"><i></i>Installed</span></div><div class="card store-card"><div class="store-icon">◎</div><b>Node Dashboard</b><p>Local management interface for your AWEp2P node.</p><span class="status"><i></i>Installed</span></div><div class="card store-card"><div class="store-icon">+</div><b>Modules</b><p>Future AWEStore packages will be managed here.</p><span class="pill">Catalog API needed</span></div></div>';
 } else if(k==="security"){
  body='<div class="grid">'+card("Identity",live.node_id,"Public node identity")+card("Transport",live.transport,"Network layer")+card("Peer count",live.peers.length,"Known peers")+card("Trust","Core-managed","No UI-only security state")+'</div>'+
  panel("Security model",'<div class="list"><div class="list-row"><span>Node identity</span><b>Rust core</b></div><div class="list-row"><span>Authentication</span><b>Core protocol</b></div><div class="list-row"><span>Transport</span><b>'+esc(live.transport)+'</b></div><div class="list-row"><span>Secrets</span><b>Local identity vault</b></div></div>');
 } else if(k==="diagnostics"){
  body='<div class="grid">'+card("Node","'+esc(live.status).toUpperCase()+'","Live API status")+card("API",live.ui,"Dashboard connection")+card("Peers",live.peers.length,"Known peers")+card("Endpoint",live.node_address,"Listen endpoint")+'</div>'+
  panel("Runtime diagnostics",'<div class="terminal"><div>$ GET /api/status</div><div class="green">200 · '+esc(live.status)+'</div><div>$ node</div><div class="green">'+esc(live.node_id)+'</div><div>$ peers</div><div class="green">'+live.peers.length+' known peer(s)</div></div>','<button class="primary" id="healthBtn">Run health check</button>')+
  panel("Result",'<div id="healthResult" class="notice">Press “Run health check” to query the live node health endpoint.</div>');
 } else if(k==="settings"){
  body=panel("Node API",'<div class="peer-form"><input id="apiBase" placeholder="http://192.168.1.10:41800" value="'+esc(apiBase())+'"><button class="primary" id="saveApi">Save & Test</button></div><div class="muted" style="margin-top:8px">Leave empty for the bundled local desktop node. Android can point to a reachable node API.</div>')+
  panel("Interface",'<div class="list"><div class="list-row"><span>Theme</span><b>System UI</b></div><div class="list-row"><span>Auto refresh</span><b>5 seconds</b></div><div class="list-row"><span>API mode</span><b>'+esc(apiBase()||"Local")+'</b></div></div>');
 }
 view.innerHTML='<div class="content"><div class="hero"><div><h1>'+p[0]+'</h1><p>'+p[1]+'</p></div><div class="actions"><button class="secondary" id="refreshBtn">Refresh</button></div></div>'+body+'</div>';
 bind(k);
}
function bind(k){
 const r=document.getElementById("refreshBtn");if(r)r.onclick=async()=>{await refresh();render(k)};
 const g=document.getElementById("goNetwork");if(g)g.onclick=()=>render("network");
 const pr=document.getElementById("peerRefresh");if(pr)pr.onclick=async()=>{await refresh();render("network")};
 const cb=document.getElementById("connectBtn");if(cb)cb.onclick=async()=>{const a=document.getElementById("peerAddress").value.trim();if(!a)return toast("Enter a node address");cb.disabled=true;try{const x=await api("/api/connect?address="+encodeURIComponent(a),{method:"POST"});toast("Bootstrap complete");await refresh();render("network")}catch(e){toast("Connection failed: "+e.message)}finally{cb.disabled=false}};
 const sa=document.getElementById("saveApi");if(sa)sa.onclick=async()=>{localStorage.setItem("aweApiBase",document.getElementById("apiBase").value.trim().replace(/\/$/,""));await refresh();render("settings");toast("API endpoint saved")};
 const ss=document.getElementById("saveStorage");if(ss)ss.onclick=()=>{localStorage.setItem("aweStoragePath",document.getElementById("storagePath").value.trim());toast("Storage preference saved")};
 const hb=document.getElementById("healthBtn");if(hb)hb.onclick=async()=>{const box=document.getElementById("healthResult");try{const x=await api("/api/health");box.innerHTML='<span class="status"><i></i>Health check passed</span><div class="detail" style="margin-top:8px">'+esc(JSON.stringify(x))+'</div>'}catch(e){box.textContent="Health check failed: "+e.message}};
 const sm=document.getElementById("sendMsg");if(sm)sm.onclick=async()=>{const recipient=document.getElementById("msgRecipient").value.trim(),message=document.getElementById("msgText").value.trim(),state=document.getElementById("msgState");if(!recipient||!message)return toast("Recipient and message are required");sm.disabled=true;try{const r=await api("/api/messenger/send",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({recipient,text:message})});state.textContent="Queued: "+r.message.id;document.getElementById("msgText").value="";await loadMessenger()}catch(e){state.textContent="Send failed: "+e.message}finally{sm.disabled=false}};
}
function toast(t){const e=document.createElement("div");e.textContent=t;e.style="position:fixed;right:22px;bottom:22px;background:#111829;color:#fff;padding:11px 15px;border-radius:9px;font-size:11px;z-index:10";document.body.appendChild(e);setTimeout(()=>e.remove(),2200)}
navs.forEach(n=>n.addEventListener("click",()=>render(n.dataset.view)));
(async()=>{await refresh();render("dashboard");setInterval(async()=>{await refresh();if(title.textContent===pages.dashboard[0])render("dashboard")},5000)})();