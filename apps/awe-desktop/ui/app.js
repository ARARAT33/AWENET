const view = document.getElementById("view");
const title = document.getElementById("pageTitle");
const navs = [...document.querySelectorAll(".nav[data-view]")];

const pages = {
  dashboard:["Overview","Your AWE network at a glance"],
  node:["My Node","Manage your node and local storage"],
  profile:["My Profile","Your private local profile"],
  sites:["My Sites","Create, open, edit and remove your sites"],
  saved:["Saved","Your saved resources"],
  browser:["AWENET Browser","Search AWENET and open sites, files and resources"],
  translate:["Translate","Translate text and pages with Google Translate"],
  network:["Connections","Connect to devices without exposing your user profile"],
  federation:["AWENET","Node, Data Centre and Data Group configuration"],
  storage:["Node Storage","Choose storage and manage encrypted files"],
  messenger:["Messenger","Messages, voice notes, calls and media"],
  groups:["Groups & Channels","Automatic groups, channels and communities"],
  resources:["Resources","Choose CPU, GPU, SSD, HDD and bandwidth contribution"],
  developers:["Developers","Build mini apps, sites and AWE services"],
  store:["AWEStore","Packages and installed modules"],
  security:["Security","Identity, transport and policy"],
  diagnostics:["Diagnostics","Node health and runtime diagnostics"],
  settings:["Settings","Application configuration"]
};

let live = {
  status:"starting",
  device_id:"—",
  node_address:"—",
  transport:"—",
  peers:[],
  storage:{},
  security:{},
  federation:{}
};

function apiBase(){
  return (localStorage.getItem("aweApiBase") || "http://127.0.0.1:41800").replace(/\/$/,"");
}

async function api(path, options={}){
  const response = await fetch(apiBase()+path, options);
  const text = await response.text();
  let data = {};
  try { data = text ? JSON.parse(text) : {}; } catch { data = {message:text}; }
  if(!response.ok) throw new Error(data.error || data.message || ("HTTP "+response.status));
  return data;
}

function esc(value){
  return String(value ?? "").replace(/[&<>'"]/g, ch => ({
    "&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;",'"':"&quot;"
  }[ch]));
}

function store(key, fallback){
  try { return JSON.parse(localStorage.getItem(key) || JSON.stringify(fallback)); }
  catch { return fallback; }
}

function save(key, value){
  localStorage.setItem(key, JSON.stringify(value));
}

function toast(message){
  const old = document.querySelector(".toast");
  if(old) old.remove();
  const el = document.createElement("div");
  el.className = "toast";
  el.textContent = message;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 2600);
}

function fmt(value){
  const n = Number(value) || 0;
  if(n < 1024) return n+" B";
  if(n < 1048576) return (n/1024).toFixed(1)+" KB";
  if(n < 1073741824) return (n/1048576).toFixed(1)+" MB";
  return (n/1073741824).toFixed(2)+" GB";
}

function card(label,value,note=""){
  return '<div class="card"><div class="metric-label">'+esc(label)+'</div><div class="metric">'+esc(value)+'</div><div class="muted">'+esc(note)+'</div></div>';
}

function panel(heading, body, action=""){
  return '<div class="section panel"><div class="section-head"><h2>'+esc(heading)+'</h2>'+action+'</div>'+body+'</div>';
}

function setConnection(online){
  const dot=document.getElementById("sideDot");
  const state=document.getElementById("sideState");
  const transport=document.getElementById("sideTransport");
  const badge=document.getElementById("apiBadge");
  if(dot) dot.classList.toggle("online",online);
  if(state) state.textContent=online?"Node online":"Node offline";
  if(transport) transport.textContent=online?(live.transport||"AWE transport"):"API unavailable";
  if(badge) badge.textContent="API · "+(online?(live.node_address||"online"):"offline");
}

async function refresh(){
  try{
    const [status,node,storage,security,federation] = await Promise.all([
      api("/api/status"), api("/api/node"), api("/api/storage"),
      api("/api/security"), api("/api/federation"), api("/api/resources").catch(()=>({}))
    ]);
    live={...live,...status,...node,storage,security,federation,resources};
    setConnection(true);
    return true;
  }catch{
    live={...live,status:"offline",peers:[]};
    setConnection(false);
    return false;
  }
}

function peerRows(){
  if(!live.peers?.length) return '<tr><td colspan="4" class="empty">No connected devices.</td></tr>';
  return live.peers.map(peer =>
    '<tr><td><div class="peer"><i class="peer-dot"></i><b>'+esc(peer.device_id||peer.id||"device")+
    '</b></div></td><td>'+esc(peer.address||"—")+'</td><td>'+esc(peer.last_seen||"—")+
    '</td><td><span class="status"><i></i>Connected</span></td></tr>'
  ).join("");
}

function renderDashboard(){
  return '<div class="grid">'+
    card("Node",String(live.status||"offline").toUpperCase())+
    card("Device ID",live.device_id||"—","Private connection identifier")+
    card("Connected devices",live.peers?.length||0,"Authenticated links")+
    card("Storage",fmt(live.storage?.used_bytes),"Node-managed disk")+
    '</div>'+
    panel("Network",
      '<table class="table"><thead><tr><th>Device</th><th>Endpoint</th><th>Last seen</th><th>State</th></tr></thead><tbody>'+
      peerRows()+'</tbody></table>',
      '<button type="button" class="secondary" data-action="view" data-value="network">Manage</button>')+
    panel("Quick actions",
      '<div class="quick-actions">'+
      '<button type="button" class="primary" data-action="view" data-value="node">Configure node</button>'+
      '<button type="button" class="secondary" data-action="view" data-value="storage">Choose storage</button>'+
      '<button type="button" class="secondary" data-action="view" data-value="sites">Manage sites</button>'+
      '<button type="button" class="secondary" data-action="view" data-value="saved">Saved</button>'+
      '</div>');
}

function renderNode(){
  const folder=localStorage.getItem("aweNodeFolderName")||"Not selected";
  return '<div class="grid">'+
    card("Status",String(live.status||"offline").toUpperCase())+
    card("Device ID",live.device_id||"—","Only connection metadata is shown")+
    card("Endpoint",live.node_address||"—")+
    card("Storage",folder,"Selected for this node")+
    '</div>'+
    panel("Node storage",
      '<div class="notice">Choose a dedicated or empty folder. Selected files are imported into node storage and served by the node.</div>'+
      '<div class="peer-form" style="margin-top:10px">'+
      '<button type="button" class="primary" data-action="choose-node-folder">Choose folder / disk</button>'+
      '<input id="folderFiles" type="file" webkitdirectory directory multiple hidden>'+
      '</div>'+
      '<div id="folderResult" class="notice" style="margin-top:10px">'+esc(folder)+'</div>')+
    panel("Identity boundary",
      '<div class="list">'+
      '<div class="list-row"><span>Peer action</span><b>connect</b></div>'+
      '<div class="list-row"><span>Peer-visible ID</span><b>'+esc(live.device_id||"device")+'</b></div>'+
      '<div class="list-row"><span>User profile</span><b>private</b></div>'+
      '<div class="list-row"><span>AWE identity</span><b>local cryptographic identity</b></div>'+
      '</div>');
}

function renderProfile(){
  const p=store("aweProfile",{displayName:"",bio:"",avatar:"A"});
  return panel("Profile",
    '<div class="profile-card"><div class="avatar-large">'+esc((p.avatar||"A").slice(0,1).toUpperCase())+
    '</div><div><h2>'+esc(p.displayName||"AWE user")+
    '</h2><div class="muted">Private profile · stored on this device</div></div></div>'+
    '<div class="form-grid">'+
    '<label>Display name<input id="profileName" maxlength="80" value="'+esc(p.displayName)+'"></label>'+
    '<label>Avatar<input id="profileAvatar" maxlength="1" value="'+esc(p.avatar)+'"></label>'+
    '<label class="wide">Bio<textarea id="profileBio" maxlength="280">'+esc(p.bio)+'</textarea></label>'+
    '</div>'+
    '<button type="button" class="primary" data-action="save-profile">Save profile</button>'+
    '<div class="notice" style="margin-top:10px">This profile is not the peer connection identity.</div>');
}

function renderSites(){
  const sites=store("aweSites",[]);
  const rows=sites.length ? sites.map((s,i)=>
    '<div class="site-row"><div><b>'+esc(s.name)+'</b><div class="muted">'+esc(s.url)+'</div><div class="detail">'+esc(s.description)+'</div></div>'+
    '<div class="row-actions"><button type="button" class="secondary" data-action="open-site" data-index="'+i+'">Open</button>'+
    '<button type="button" class="secondary danger" data-action="delete-site" data-index="'+i+'">Delete</button></div></div>'
  ).join("") : '<div class="empty">No sites yet.</div>';
  return panel("Add site",
    '<div class="form-grid"><label>Name<input id="siteName" placeholder="My site"></label>'+
    '<label>Address<input id="siteUrl" placeholder="awe://site or https://…"></label>'+
    '<label class="wide">Description<textarea id="siteDesc"></textarea></label></div>'+
    '<button type="button" class="primary" data-action="add-site">Add site</button>')+
    panel("My sites",'<div class="site-list">'+rows+'</div>');
}

function renderSaved(){
  const items=store("aweSaved",[]);
  const rows=items.length ? items.map((s,i)=>
    '<div class="site-row"><div><b>'+esc(s.title)+'</b><div class="muted">'+esc(s.url)+'</div></div>'+
    '<div class="row-actions"><button type="button" class="secondary" data-action="open-saved" data-index="'+i+'">Open</button>'+
    '<button type="button" class="secondary danger" data-action="delete-saved" data-index="'+i+'">Delete</button></div></div>'
  ).join("") : '<div class="empty">Nothing saved yet.</div>';
  return panel("Save resource",
    '<div class="peer-form"><input id="saveTitle" placeholder="Title"><input id="saveUrl" placeholder="AWE resource or URL">'+
    '<button type="button" class="primary" data-action="add-saved">Save</button></div>')+
    panel("Saved resources",'<div class="site-list">'+rows+'</div>');
}

function renderNetwork(){
  return panel("Connect",
    '<div class="peer-form"><input id="peerAddress" placeholder="127.0.0.1:41000">'+
    '<button type="button" class="primary" data-action="connect">Connect</button></div>'+
    '<div class="notice" style="margin-top:10px"><b>connect</b> is the peer-facing action. The displayed ID is a device connection ID.</div>')+
    panel("Connected devices",
      '<table class="table"><thead><tr><th>Device</th><th>Endpoint</th><th>Last seen</th><th>State</th></tr></thead><tbody>'+
      peerRows()+'</tbody></table>',
      '<button type="button" class="secondary" data-action="refresh">Refresh</button>');
}

function renderFederation(){
  return panel("AWENET",
    '<div class="grid">'+card("Node",live.device_id||"—","Local device")+
    card("Data centre",live.federation?.data_centre_id||"Not joined")+
    card("Data group",live.federation?.data_group_id||"Not joined")+
    card("Live links",live.peers?.length||0)+'</div>')+
    panel("Configuration",
      '<div class="notice">Generate signed AWENET configuration files. Runtime membership remains controlled by the node.</div>'+
      '<div class="peer-form" style="margin-top:10px"><input id="fedNodeName" placeholder="Data centre name"><input id="fedEndpoint" placeholder="'+esc(live.node_address||"endpoint")+'"><button type="button" class="primary" data-action="gen-node">Generate .awenode</button></div>'+
      '<div class="peer-form" style="margin-top:8px"><input id="fedDcId" placeholder="Data centre ID"><input id="fedDcName" placeholder="Data centre name"><button type="button" class="primary" data-action="gen-dc">Generate .awedc</button></div>'+
      '<div class="peer-form" style="margin-top:8px"><input id="fedGroupName" placeholder="Data group name"><input id="fedCentres" placeholder="dc-1,dc-2"><button type="button" class="primary" data-action="gen-dgc">Generate .dgc</button></div>');
}

function renderStorage(){
  return '<div class="grid">'+card("Used",fmt(live.storage?.used_bytes))+
    card("Free",fmt(live.storage?.free_bytes))+
    card("Objects",live.storage?.objects||0)+
    card("Healthy replicas",live.storage?.healthy_replicas||0)+'</div>'+
    panel("Import from disk",
      '<div class="notice">Choose a folder and import files into encrypted node storage.</div>'+
      '<div class="peer-form" style="margin-top:10px"><button type="button" class="primary" data-action="choose-storage-folder">Choose folder</button>'+
      '<input id="storageFolderFiles" type="file" webkitdirectory directory multiple hidden></div>'+
      '<div id="storageFolderResult" class="notice" style="margin-top:10px">No folder selected.</div>')+
    panel("File transfer",
      '<div class="peer-form"><input type="file" id="storageFile"><button type="button" class="primary" data-action="upload">Upload & Replicate</button></div>'+
      '<div id="storageUploadResult" class="notice" style="margin-top:10px">Encrypted locally before replication.</div>'+
      '<div class="peer-form" style="margin-top:8px"><input id="downloadFileId" placeholder="64-hex file ID"><button type="button" class="primary" data-action="download">Reconstruct & Download</button></div>'+
      '<div id="storageDownloadResult" class="notice" style="margin-top:10px">Remote shards are reconstructed and decrypted locally.</div>');
}

function renderMessenger(){
  return panel("Messenger",
    '<div class="peer-form"><input id="msgRecipient" placeholder="Recipient"><input id="msgText" placeholder="Message">'+
    '<button type="button" class="primary" data-action="send-message">Send</button></div>'+
    '<div id="msgState" class="muted" style="margin-top:8px">Delivery requires acknowledgement.</div>'+
    '<div id="messageList" class="list" style="margin-top:12px"><div class="empty">Loading…</div></div>');
}

function renderStore(){
  return '<div class="store-grid">'+
    '<div class="card store-card"><div class="store-icon">◈</div><b>AWE Core</b><p>Networking, identity, routing and node services.</p><span class="status"><i></i>Installed</span></div>'+
    '<div class="card store-card"><div class="store-icon">◎</div><b>Node Dashboard</b><p>Local management interface.</p><span class="status"><i></i>Installed</span></div>'+
    '<div class="card store-card"><div class="store-icon">+</div><b>AWEStore</b><p>Signed packages and capability-scoped modules.</p><span class="pill">Ready</span></div>'+
    '</div>';
}

function renderSecurity(){
  return '<div class="grid">'+card("Identity","Protected","Local cryptographic identity")+
    card("Transport",live.transport||"—")+card("Replay","Enabled")+card("Policy","Fail closed")+'</div>'+
    panel("Privacy boundary",'<div class="notice">User profile data is separate from peer connection metadata. Peers see connection/device information, not the local UI profile.</div>');
}

function renderDiagnostics(){
  return panel("Runtime diagnostics",
    '<div class="terminal"><div>$ node</div><div class="green">'+esc(live.device_id||"—")+
    '</div><div>$ peers</div><div class="green">'+(live.peers?.length||0)+' connected device(s)</div></div>',
    '<button type="button" class="primary" data-action="health">Run health check</button>')+
    panel("Result",'<div id="healthResult" class="notice">No check run yet.</div>');
}

function renderSettings(){
  return panel("Node API",
    '<div class="peer-form"><input id="apiBase" value="'+esc(apiBase())+'"><button type="button" class="primary" data-action="save-api">Save & Test</button></div>')+
    panel("Preferences",
      '<div class="list"><div class="list-row"><span>Profile visibility</span><b>Private</b></div>'+
      '<div class="list-row"><span>Peer display</span><b>Device ID only</b></div>'+
      '<div class="list-row"><span>Auto refresh</span><b>5 seconds</b></div></div>');
}

function render(key="dashboard"){
  const page=pages[key]||pages.dashboard;
  navs.forEach(n=>n.classList.toggle("active",n.dataset.view===key));
  title.textContent=page[0];
  let body="";
  if(key==="dashboard") body=renderDashboard();
  else if(key==="node") body=renderNode();
  else if(key==="profile") body=renderProfile();
  else if(key==="sites") body=renderSites();
  else if(key==="saved") body=renderSaved();
  else if(key==="network") body=renderNetwork();
  else if(key==="federation") body=renderFederation();
  else if(key==="storage") body=renderStorage();
  else if(key==="messenger") body=renderMessenger();
  else if(key==="store") body=renderStore();
  else if(key==="security") body=renderSecurity();
  else if(key==="diagnostics") body=renderDiagnostics();
  else if(key==="settings") body=renderSettings();
  view.innerHTML='<div class="content"><div class="hero"><div><h1>'+page[0]+'</h1><p>'+page[1]+'</p></div>'+
    '<div class="actions"><button type="button" class="secondary" data-action="refresh">Refresh</button></div></div>'+body+'</div>';
  if(key==="messenger") loadMessenger();
}

async function loadMessenger(){
  const box=document.getElementById("messageList");
  if(!box) return;
  try{
    const data=await api("/api/messenger");
    box.innerHTML=(data.messages||[]).length
      ? data.messages.map(m=>'<div class="list-row"><span>'+esc(m.recipient)+'</span><b>'+esc(m.state)+'</b><span>'+esc(m.text)+'</span></div>').join("")
      : '<div class="empty">No messages.</div>';
  }catch{ box.innerHTML='<div class="empty">Messenger API unavailable.</div>'; }
}

async function importFiles(files, box){
  let ok=0,fail=0;
  for(const file of [...files].slice(0,100)){
    if(file.size>64*1024*1024){fail++;continue;}
    try{
      const bytes=new Uint8Array(await file.arrayBuffer());
      let hex="";
      for(const b of bytes) hex+=b.toString(16).padStart(2,"0");
      await api("/api/storage/put",{method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({filename:file.webkitRelativePath||file.name,data_hex:hex})});
      ok++;
    }catch{fail++;}
  }
  if(box) box.textContent="Imported "+ok+" file(s)"+(fail?" · "+fail+" failed":"")+"."; 
  await refresh();
}

async function importDirectory(handle, box, prefix=""){
  let count=0,failed=0;
  async function walk(dir,path){
    for await(const entry of dir.values()){
      if(entry.kind==="directory") await walk(entry,path+entry.name+"/");
      else if(entry.kind==="file"){
        try{
          const file=await entry.getFile();
          if(file.size>64*1024*1024){failed++;continue;}
          const bytes=new Uint8Array(await file.arrayBuffer());
          let hex="";
          for(const b of bytes) hex+=b.toString(16).padStart(2,"0");
          await api("/api/storage/put",{method:"POST",headers:{"Content-Type":"application/json"},
            body:JSON.stringify({filename:path+file.name,data_hex:hex})});
          count++;
        }catch{failed++;}
      }
    }
  }
  await walk(handle,prefix);
  if(box) box.textContent="Synced "+count+" file(s)"+(failed?" · "+failed+" skipped":"")+".";
  await refresh();
}

async function generateConfig(kind,payload){
  try{
    const data=await api("/api/federation/generate",{method:"POST",headers:{"Content-Type":"application/json"},
      body:JSON.stringify({kind,...payload})});
    if(data.status!=="generated") throw new Error(data.error||"Generation failed");
    const blob=URL.createObjectURL(new Blob([data.content],{type:"application/json"}));
    const a=document.createElement("a");a.href=blob;a.download=data.filename||"awe-config.json";a.click();
    setTimeout(()=>URL.revokeObjectURL(blob),1000);
    toast("Generated "+(data.filename||"configuration"));
  }catch(e){toast(e.message||"Generation failed");}
}

document.addEventListener("click",async event=>{
  const nav=event.target.closest(".nav[data-view]");
  if(nav){event.preventDefault();render(nav.dataset.view);return;}
  const button=event.target.closest("[data-action]");
  if(!button) return;
  event.preventDefault();
  const action=button.dataset.action;
  try{
    if(action==="view"){render(button.dataset.value);return;}
    if(action==="refresh"){const current=Object.keys(pages).find(k=>pages[k][0]===title.textContent)||"dashboard";await refresh();render(current);return;}
    if(action==="save-profile"){
      save("aweProfile",{displayName:document.getElementById("profileName").value.trim(),avatar:document.getElementById("profileAvatar").value.trim()||"A",bio:document.getElementById("profileBio").value.trim()});
      toast("Profile saved");render("profile");return;
    }
    if(action==="add-site"){
      const name=document.getElementById("siteName").value.trim(),url=document.getElementById("siteUrl").value.trim();
      if(!name||!url){toast("Name and address are required");return;}
      const items=store("aweSites",[]);items.push({name,url,description:document.getElementById("siteDesc").value.trim(),created:Date.now()});save("aweSites",items);toast("Site added");render("sites");return;
    }
    if(action==="delete-site"){
      const items=store("aweSites",[]);items.splice(Number(button.dataset.index),1);save("aweSites",items);render("sites");return;
    }
    if(action==="open-site"){
      const item=store("aweSites",[])[Number(button.dataset.index)];
      if(item?.url) window.open(item.url,"_blank"); return;
    }
    if(action==="add-saved"){
      const titleValue=document.getElementById("saveTitle").value.trim(),url=document.getElementById("saveUrl").value.trim();
      if(!titleValue||!url){toast("Title and address are required");return;}
      const items=store("aweSaved",[]);items.push({title:titleValue,url,created:Date.now()});save("aweSaved",items);toast("Saved");render("saved");return;
    }
    if(action==="delete-saved"){
      const items=store("aweSaved",[]);items.splice(Number(button.dataset.index),1);save("aweSaved",items);render("saved");return;
    }
    if(action==="open-saved"){
      const item=store("aweSaved",[])[Number(button.dataset.index)];
      if(item?.url) window.open(item.url,"_blank"); return;
    }
    if(action==="choose-node-folder"){
      const input=document.getElementById("folderFiles");
      if(window.showDirectoryPicker){
        try{
          const handle=await window.showDirectoryPicker({mode:"read"});
          localStorage.setItem("aweNodeFolderName",handle.name);
          const box=document.getElementById("folderResult");box.textContent="Syncing "+handle.name+"…";
          await importDirectory(handle,box);toast("Node storage synced");
        }catch(e){toast(e.message||"Folder selection cancelled");}
      }else input.click();
      return;
    }
    if(action==="choose-storage-folder"){document.getElementById("storageFolderFiles").click();return;}
    if(action==="connect"){
      const address=document.getElementById("peerAddress").value.trim();
      if(!address){toast("Enter a node address");return;}
      button.disabled=true;
      try{await api("/api/connect?address="+encodeURIComponent(address),{method:"POST"});toast("Connect requested");await refresh();render("network");}
      finally{button.disabled=false;}
      return;
    }
    if(action==="upload"){
      const file=document.getElementById("storageFile").files[0];
      if(!file){toast("Choose a file");return;}
      await importFiles([file],document.getElementById("storageUploadResult"));return;
    }
    if(action==="download"){
      const id=document.getElementById("downloadFileId").value.trim();
      const box=document.getElementById("storageDownloadResult");
      if(!/^[0-9a-fA-F]{64}$/.test(id)){toast("Enter a valid 64-hex file ID");return;}
      const data=await api("/api/storage/get?file_id="+encodeURIComponent(id));
      const raw=data.data_hex||"",bytes=new Uint8Array(raw.length/2);
      for(let i=0;i<bytes.length;i++) bytes[i]=parseInt(raw.slice(i*2,i*2+2),16);
      const a=document.createElement("a"),url=URL.createObjectURL(new Blob([bytes]));
      a.href=url;a.download=data.filename||"awep2p-file";a.click();URL.revokeObjectURL(url);
      box.textContent="Downloaded "+(data.filename||"file");return;
    }
    if(action==="send-message"){
      const recipient=document.getElementById("msgRecipient").value.trim(),textValue=document.getElementById("msgText").value.trim();
      if(!recipient||!textValue){toast("Recipient and message are required");return;}
      try{
        const data=await api("/api/messenger/send",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify({recipient,text:textValue})});
        document.getElementById("msgState").textContent="Delivery: "+(data.status||"sent");await loadMessenger();
      }catch(e){document.getElementById("msgState").textContent="Send failed: "+e.message;}
      return;
    }
    if(action==="save-api"){
      const value=document.getElementById("apiBase").value.trim().replace(/\/$/,"");
      localStorage.setItem("aweApiBase",value||"http://127.0.0.1:41800");
      await refresh();render("settings");toast("API endpoint saved");return;
    }
    if(action==="health"){
      const box=document.getElementById("healthResult");
      try{box.textContent="Checking…";box.textContent="Healthy · "+JSON.stringify(await api("/api/health"));}
      catch(e){box.textContent="Health check failed: "+e.message;}
      return;
    }
    if(action==="gen-node"){
      await generateConfig("awenode",{name:document.getElementById("fedNodeName").value.trim()||"AWE Data Centre",endpoint:document.getElementById("fedEndpoint").value.trim()||live.node_address,bootstrap:[]});return;
    }
    if(action==="gen-dc"){
      await generateConfig("awedc",{data_centre_id:document.getElementById("fedDcId").value.trim(),name:document.getElementById("fedDcName").value.trim()||"AWE Data Centre",endpoints:[live.node_address]});return;
    }
    if(action==="gen-dgc"){
      await generateConfig("dgc",{owner_data_centre_id:live.federation?.data_centre_id||"",name:document.getElementById("fedGroupName").value.trim()||"AWE Data Group",data_centre_ids:(document.getElementById("fedCentres").value||"").split(",").map(x=>x.trim()).filter(Boolean)});return;
    }
  }catch(e){toast(e.message||"Action failed");}
});

document.addEventListener("change",event=>{
  if(event.target.id==="folderFiles"){
    const box=document.getElementById("folderResult");
    localStorage.setItem("aweNodeFolderName",event.target.files[0]?.webkitRelativePath?.split("/")[0]||"Selected folder");
    importFiles(event.target.files,box);
  }
  if(event.target.id==="storageFolderFiles"){
    importFiles(event.target.files,document.getElementById("storageFolderResult"));
  }
});

document.getElementById("avatarBtn")?.addEventListener("click",()=>render("profile"));

render("dashboard");
refresh().then(()=>render("dashboard"));
setInterval(async()=>{
  await refresh();
  if(title.textContent===pages.dashboard[0]) render("dashboard");
},5000);
