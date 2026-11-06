/* ONECOIN Market — the AWENET Store surface: browse the catalog, sell
   products and services priced in ONECOIN, and manage escrowed orders.
   Payments always move buyer → escrow → seller; sale revenue is never
   newly minted ONECOIN. */
(()=>{
const view=document.getElementById("view");
const base=()=>location.protocol==="http:"||location.protocol==="https:"?location.origin:"http://127.0.0.1:41800";
const get=async p=>{const r=await fetch(base()+p,{cache:"no-store"});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const post=async(p,b)=>{const r=await fetch(base()+p,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(b)});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const esc=x=>String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]));
const coins=n=>(Number(n)||0).toLocaleString(undefined,{maximumFractionDigits:6})+" ONECOIN";
const kinds=["digital","service","physical"];
let browse={q:"",kind:"",category:""};

function render(){
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">ONECOIN MARKET</div><h1>AWENET Store</h1><p>Products and services priced in ONECOIN. Buyer payments are held in escrow and released to the seller only after delivery confirmation; sale revenue is a transfer, never new issuance.</p></div><div class=\"eco-orb\">◍</div></div>"+
"<div class=\"eco-tabs\"><button class=\"eco-tab active\" data-tab=\"browse\">Browse</button><button class=\"eco-tab\" data-tab=\"sell\">Sell</button><button class=\"eco-tab\" data-tab=\"orders\">My orders</button></div>"+
"<section id=\"mkt-browse\" class=\"eco-panel\"><div class=\"section panel\"><div class=\"section-head\"><h2>Catalog</h2></div><div class=\"eco-filter\"><input id=\"mktQ\" placeholder=\"Search products\"><select id=\"mktKind\"><option value=\"\">All kinds</option>"+kinds.map(k=>"<option value=\""+k+"\">"+k+"</option>").join("")+"</select><input id=\"mktCategory\" placeholder=\"Category\"><button class=\"primary\" id=\"mktApply\">Search</button></div><div id=\"mktProducts\" class=\"product-grid\"><div class=\"empty\">Loading catalog…</div></div></div></section>"+
"<section id=\"mkt-sell\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>My seller page</h2></div><div id=\"mktSellerStats\" class=\"eco-stat-grid\"></div><div class=\"eco-form-grid\"><input id=\"mktTitle\" placeholder=\"Product title (required)\"><select id=\"mktKindSel\">"+kinds.map(k=>"<option value=\""+k+"\">"+k+"</option>").join("")+"</select><input id=\"mktCategoryIn\" placeholder=\"Category (optional)\"><input id=\"mktPrice\" type=\"text\" placeholder=\"Price (ONECOIN, decimal)\"><input id=\"mktStock\" type=\"number\" min=\"1\" placeholder=\"Stock (empty = unlimited)\"></div><textarea id=\"mktDescription\" rows=\"3\" placeholder=\"Description (optional)\"></textarea><button class=\"primary\" id=\"mktList\">List product</button><div id=\"mktListState\" class=\"notice\"></div><div id=\"mktMyProducts\" class=\"task-list\"></div></div></section>"+
"<section id=\"mkt-orders\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>My orders</h2></div><div class=\"notice\">As a buyer you confirm delivery to release the escrow, or open a dispute for the founder to settle. Digital goods and services are delivered directly between people; the escrow only guards the payment.</div><div id=\"mktOrders\" class=\"task-list\"><div class=\"empty\">Loading orders…</div></div></div></section>";
bindTabs();loadProducts();loadSeller();loadOrders();
document.getElementById("mktApply").onclick=()=>{browse.q=document.getElementById("mktQ").value.trim();browse.kind=document.getElementById("mktKind").value;browse.category=document.getElementById("mktCategory").value.trim();loadProducts();};
document.getElementById("mktList").onclick=async()=>{const s=document.getElementById("mktListState");const body={title:document.getElementById("mktTitle").value.trim(),kind:document.getElementById("mktKindSel").value,category:document.getElementById("mktCategoryIn").value.trim(),description:document.getElementById("mktDescription").value.trim()};const priceRaw=document.getElementById("mktPrice").value.trim();if(!priceRaw){s.textContent="A price in ONECOIN is required.";return}body.price_coins=priceRaw;const stock=document.getElementById("mktStock").value;if(stock!=="")body.stock=Number(stock);try{const r=await post("/api/storefront/products",body);s.textContent="Listed as product #"+r.product_id+". It is now visible in the catalog.";loadSeller()}catch(e){s.textContent="Listing failed: "+e.message}};
}

function bindTabs(){
document.querySelectorAll(".eco-tab").forEach(b=>b.onclick=()=>{document.querySelectorAll(".eco-tab").forEach(x=>x.classList.remove("active"));b.classList.add("active");document.querySelectorAll(".eco-panel").forEach(x=>x.hidden=true);document.getElementById("mkt-"+b.dataset.tab).hidden=false;});
document.querySelectorAll(".eco-tab").forEach(b=>{if(b.dataset.tab==="sell")b.addEventListener("click",loadSeller);if(b.dataset.tab==="orders")b.addEventListener("click",loadOrders);});
}

async function loadProducts(){
const box=document.getElementById("mktProducts");
if(!box)return;
try{
const q=new URLSearchParams();
if(browse.q)q.set("q",browse.q);
if(browse.kind)q.set("kind",browse.kind);
if(browse.category)q.set("category",browse.category);
const d=await get("/api/storefront/products"+(q.toString()?"?"+q.toString():""));
box.innerHTML=(d.products||[]).length?(d.products||[]).map(p=>"<div class=\"product-card\"><div class=\"task-head\"><b>"+esc(p.title)+"</b><span class=\"badge\">"+esc(p.kind)+"</span></div><p>"+esc(p.description||"")+"</p><div class=\"task-meta\"><span>"+esc(p.category||"general")+"</span><span>"+coins(p.price&&p.price.coins)+"</span><span>"+(p.stock==null?"unlimited stock":(p.stock>0?p.stock+" left":"sold out"))+"</span><span>"+(p.sold_units||0)+" sold</span></div><div class=\"eco-actions\"><input class=\"mkt-units\" data-id=\""+p.id+"\" type=\"number\" min=\"1\" value=\"1\"><button class=\"primary mkt-buy\" data-id=\""+p.id+"\">Buy with ONECOIN</button></div></div>").join(""):"<div class=\"empty\">No products match. The catalog fills as sellers list products.</div>";
box.querySelectorAll(".mkt-buy").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const units=Number(box.querySelector(".mkt-units[data-id=\""+id+"\"]").value)||1;try{const r=await post("/api/storefront/orders",{product_id:Number(id),units});alert("Order #"+r.order_id+" funded ("+coins(r.amount&&r.amount.coins)+"). Held in escrow until you confirm delivery.");loadOrders()}catch(e){alert("Purchase failed: "+e.message)}});
}catch(e){box.innerHTML="<div class=\"empty\">Catalog unavailable: "+esc(e.message)+"</div>";}
}

async function loadSeller(){
const statsBox=document.getElementById("mktSellerStats");
const listBox=document.getElementById("mktMyProducts");
if(!statsBox||!listBox)return;
try{
const d=await get("/api/storefront/seller");
statsBox.innerHTML="<div class=\"card eco-stat\"><div class=\"metric-label\">Products</div><div class=\"metric\">"+esc(d.stats.products||0)+"</div><div class=\"muted\">Active: "+esc(d.stats.active_products||0)+"</div></div><div class=\"card eco-stat\"><div class=\"metric-label\">Sold units</div><div class=\"metric\">"+esc(d.stats.sold_units||0)+"</div><div class=\"muted\">Escrow releases on confirmation</div></div>";
listBox.innerHTML=(d.products||[]).length?(d.products||[]).map(p=>"<div class=\"task-card\"><div class=\"task-head\"><b>"+esc(p.title)+"</b><span class=\"badge "+(p.active?"badge-published":"badge-cancelled")+"\">"+(p.active?"active":"inactive")+"</span></div><div class=\"task-meta\"><span>"+esc(p.kind)+"</span><span>"+coins(p.price&&p.price.coins)+"</span><span>"+(p.stock==null?"unlimited":p.stock+" left")+"</span><span>"+(p.sold_units||0)+" sold</span></div><div class=\"eco-form-grid\"><input class=\"mkt-up-price\" data-id=\""+p.id+"\" type=\"text\" placeholder=\"New price (ONECOIN)\"><input class=\"mkt-up-stock\" data-id=\""+p.id+"\" type=\"number\" min=\"1\" placeholder=\"New stock (empty = unlimited)\"></div><div class=\"eco-actions\"><button class=\"primary mkt-update\" data-id=\""+p.id+"\" data-title=\""+esc(p.title)+"\" data-desc=\""+esc(p.description||"")+"\" data-cat=\""+esc(p.category||"")+"\">Save changes</button><button class=\"mkt-deactivate\" data-id=\""+p.id+"\">Deactivate</button></div></div>").join(""):"<div class=\"empty\">You have no listings yet. Products you list appear in the catalog immediately.</div>";
listBox.querySelectorAll(".mkt-update").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const body={title:b.dataset.title,description:b.dataset.desc,category:b.dataset.cat,active:true};const priceRaw=listBox.querySelector(".mkt-up-price[data-id=\""+id+"\"]").value.trim();if(priceRaw)body.price_coins=priceRaw;const stock=listBox.querySelector(".mkt-up-stock[data-id=\""+id+"\"]").value;body.stock=stock===""?null:Number(stock);try{await post("/api/storefront/products/"+id+"/update",body);loadSeller()}catch(e){alert("Update failed: "+e.message)}});
listBox.querySelectorAll(".mkt-deactivate").forEach(b=>b.onclick=async()=>{try{await post("/api/storefront/products/"+b.dataset.id+"/deactivate",{});loadSeller()}catch(e){alert("Deactivate failed: "+e.message)}});
}catch(e){listBox.innerHTML="<div class=\"empty\">Seller page unavailable: "+esc(e.message)+"</div>";}
}

async function loadOrders(){
const box=document.getElementById("mktOrders");
if(!box)return;
try{
const d=await get("/api/storefront/orders");
box.innerHTML=(d.orders||[]).length?(d.orders||[]).map(o=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+o.order_id+" · "+esc(o.title)+"</b><span class=\"badge badge-"+esc(o.status)+"\">"+esc(o.status)+"</span></div><div class=\"task-meta\"><span>you are the "+esc(o.role)+"</span><span>"+coins(o.amount&&o.amount.coins)+"</span><span>fee "+coins(o.fee&&o.fee.coins)+"</span><span>buyer "+esc(String(o.buyer||"").slice(0,10))+"…</span><span>seller "+esc(String(o.seller||"").slice(0,10))+"…</span></div>"+(o.disputed?"<p><b>Dispute:</b> "+esc(o.dispute_reason||"")+" — awaiting founder ruling.</p>":"")+"<div class=\"eco-actions\">"+(o.role==="buyer"&&o.status==="held"&&!o.disputed?"<button class=\"primary mkt-confirm\" data-id=\""+o.order_id+"\">Confirm delivery (release escrow)</button><button class=\"mkt-dispute\" data-id=\""+o.order_id+"\">Open dispute</button>":"")+"</div></div>").join(""):"<div class=\"empty\">No orders yet. Purchases appear here with their escrow status.</div>";
box.querySelectorAll(".mkt-confirm").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/storefront/orders/"+b.dataset.id+"/confirm",{});alert("Escrow released. Seller received "+coins(r.seller_paid&&r.seller_paid.coins)+".");loadOrders()}catch(e){alert("Confirm failed: "+e.message)}});
box.querySelectorAll(".mkt-dispute").forEach(b=>b.onclick=async()=>{const reason=prompt("Describe the problem with this order:");if(!reason)return;try{await post("/api/storefront/orders/"+b.dataset.id+"/dispute",{reason});alert("Dispute opened. The founder will settle it.");loadOrders()}catch(e){alert("Dispute failed: "+e.message)}});
}catch(e){box.innerHTML="<div class=\"empty\">Orders unavailable: "+esc(e.message)+"</div>";}
}

document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="storefront"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent="ONECOIN Market";render()}}));
})();
