/* ONECOIN Market — the AWENET Store surface: browse the catalog, sell
   products and services priced in ONECOIN, and manage escrowed orders.
   Payments always move buyer → escrow → seller; sale revenue is never
   newly minted ONECOIN. All visible strings come from the bilingual
   catalog (window.AWEI18N). */
(()=>{
const view=document.getElementById("view");
const T=(k,a)=>window.AWEI18N.t(k,a);
const base=()=>location.protocol==="http:"||location.protocol==="https:"?location.origin:"http://127.0.0.1:41800";
const get=async p=>{const r=await fetch(base()+p,{cache:"no-store"});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const post=async(p,b)=>{const r=await fetch(base()+p,{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(b)});const t=await r.text();if(!r.ok)throw Error(t||("HTTP "+r.status));return JSON.parse(t)};
const esc=x=>String(x??"").replace(/[&<>'"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;","'":"&#39;","\"":"&quot;"}[c]));
const coins=n=>(Number(n)||0).toLocaleString(undefined,{maximumFractionDigits:6})+" ONECOIN";
const kinds=["digital","service","physical"];
let browse={q:"",kind:"",category:""};

function render(){
view.innerHTML="<div class=\"eco-hero\"><div><div class=\"eyebrow\">ONECOIN MARKET</div><h1>"+T("mkt_hero_title")+"</h1><p>"+T("mkt_hero_sub")+"</p></div><div class=\"eco-orb\">◍</div></div>"+
"<div class=\"eco-tabs\"><button class=\"eco-tab active\" data-tab=\"browse\">"+T("mkt_tab_browse")+"</button><button class=\"eco-tab\" data-tab=\"sell\">"+T("mkt_tab_sell")+"</button><button class=\"eco-tab\" data-tab=\"orders\">"+T("mkt_tab_orders")+"</button></div>"+
"<section id=\"mkt-browse\" class=\"eco-panel\"><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("mkt_catalog")+"</h2></div><div class=\"eco-filter\"><input id=\"mktQ\" placeholder=\""+T("mkt_search_ph")+"\"><select id=\"mktKind\"><option value=\"\">"+T("mkt_all_kinds")+"</option>"+kinds.map(k=>"<option value=\""+k+"\">"+T("kind_"+k)+"</option>").join("")+"</select><input id=\"mktCategory\" placeholder=\""+T("mkt_category_ph")+"\"><button class=\"primary\" id=\"mktApply\">"+T("mkt_search")+"</button></div><div id=\"mktProducts\" class=\"product-grid\"><div class=\"empty\">"+T("mkt_loading_catalog")+"</div></div></div></section>"+
"<section id=\"mkt-sell\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("mkt_seller_title")+"</h2></div><div id=\"mktSellerStats\" class=\"eco-stat-grid\"></div><div class=\"eco-form-grid\"><input id=\"mktTitle\" placeholder=\""+T("mkt_title_ph")+"\"><select id=\"mktKindSel\">"+kinds.map(k=>"<option value=\""+k+"\">"+T("kind_"+k)+"</option>").join("")+"</select><input id=\"mktCategoryIn\" placeholder=\""+T("mkt_category_opt")+"\"><input id=\"mktPrice\" type=\"text\" placeholder=\""+T("mkt_price_ph")+"\"><input id=\"mktStock\" type=\"number\" min=\"1\" placeholder=\""+T("mkt_stock_ph")+"\"></div><textarea id=\"mktDescription\" rows=\"3\" placeholder=\""+T("mkt_desc_ph")+"\"></textarea><button class=\"primary\" id=\"mktList\">"+T("mkt_list")+"</button><div id=\"mktListState\" class=\"notice\"></div><div id=\"mktMyProducts\" class=\"task-list\"></div></div></section>"+
"<section id=\"mkt-orders\" class=\"eco-panel\" hidden><div class=\"section panel\"><div class=\"section-head\"><h2>"+T("mkt_tab_orders")+"</h2></div><div class=\"notice\">"+T("mkt_orders_notice")+"</div><div id=\"mktOrders\" class=\"task-list\"><div class=\"empty\">"+T("mkt_loading_orders")+"</div></div></div></section>";
bindTabs();loadProducts();loadSeller();loadOrders();
document.getElementById("mktApply").onclick=()=>{browse.q=document.getElementById("mktQ").value.trim();browse.kind=document.getElementById("mktKind").value;browse.category=document.getElementById("mktCategory").value.trim();loadProducts();};
document.getElementById("mktList").onclick=async()=>{const s=document.getElementById("mktListState");const body={title:document.getElementById("mktTitle").value.trim(),kind:document.getElementById("mktKindSel").value,category:document.getElementById("mktCategoryIn").value.trim(),description:document.getElementById("mktDescription").value.trim()};const priceRaw=document.getElementById("mktPrice").value.trim();if(!priceRaw){s.textContent=T("mkt_price_required");return}body.price_coins=priceRaw;const stock=document.getElementById("mktStock").value;if(stock!=="")body.stock=Number(stock);try{const r=await post("/api/storefront/products",body);s.textContent=T("mkt_listed",{id:r.product_id});loadSeller()}catch(e){s.textContent=T("mkt_list_fail",{message:e.message})}};
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
box.innerHTML=(d.products||[]).length?(d.products||[]).map(p=>"<div class=\"product-card\"><div class=\"task-head\"><b>"+esc(p.title)+"</b><span class=\"badge\">"+T("kind_"+p.kind)+"</span></div><p>"+esc(p.description||"")+"</p><div class=\"task-meta\"><span>"+esc(p.category||T("mkt_general"))+"</span><span>"+coins(p.price&&p.price.coins)+"</span><span>"+(p.stock==null?T("mkt_unlimited"):(p.stock>0?T("mkt_left",{count:p.stock}):T("mkt_sold_out")))+"</span><span>"+T("mkt_sold",{count:p.sold_units||0})+"</span></div><div class=\"eco-actions\"><input class=\"mkt-units\" data-id=\""+p.id+"\" type=\"number\" min=\"1\" value=\"1\"><button class=\"primary mkt-buy\" data-id=\""+p.id+"\">"+T("mkt_buy")+"</button></div></div>").join(""):"<div class=\"empty\">"+T("mkt_empty_products")+"</div>";
box.querySelectorAll(".mkt-buy").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const units=Number(box.querySelector(".mkt-units[data-id=\""+id+"\"]").value)||1;try{const r=await post("/api/storefront/orders",{product_id:Number(id),units});alert(T("mkt_order_ok",{id:r.order_id,amount:coins(r.amount&&r.amount.coins)}));loadOrders()}catch(e){alert(T("mkt_buy_fail",{message:e.message}))}});
}catch(e){box.innerHTML="<div class=\"empty\">"+T("mkt_catalog_unavailable",{message:esc(e.message)})+"</div>";}
}

async function loadSeller(){
const statsBox=document.getElementById("mktSellerStats");
const listBox=document.getElementById("mktMyProducts");
if(!statsBox||!listBox)return;
try{
const d=await get("/api/storefront/seller");
statsBox.innerHTML="<div class=\"card eco-stat\"><div class=\"metric-label\">"+T("mkt_stat_products")+"</div><div class=\"metric\">"+esc(d.stats.products||0)+"</div><div class=\"muted\">"+T("mkt_stat_active",{count:d.stats.active_products||0})+"</div></div><div class=\"card eco-stat\"><div class=\"metric-label\">"+T("mkt_stat_sold")+"</div><div class=\"metric\">"+esc(d.stats.sold_units||0)+"</div><div class=\"muted\">"+T("mkt_stat_escrow")+"</div></div>";
listBox.innerHTML=(d.products||[]).length?(d.products||[]).map(p=>"<div class=\"task-card\"><div class=\"task-head\"><b>"+esc(p.title)+"</b><span class=\"badge "+(p.active?"badge-published":"badge-cancelled")+"\">"+(p.active?T("mkt_active"):T("mkt_inactive"))+"</span></div><div class=\"task-meta\"><span>"+T("kind_"+p.kind)+"</span><span>"+coins(p.price&&p.price.coins)+"</span><span>"+(p.stock==null?T("mkt_unlimited_short"):T("mkt_left",{count:p.stock}))+"</span><span>"+T("mkt_sold",{count:p.sold_units||0})+"</span></div><div class=\"eco-form-grid\"><input class=\"mkt-up-price\" data-id=\""+p.id+"\" type=\"text\" placeholder=\""+T("mkt_new_price_ph")+"\"><input class=\"mkt-up-stock\" data-id=\""+p.id+"\" type=\"number\" min=\"1\" placeholder=\""+T("mkt_new_stock_ph")+"\"></div><div class=\"eco-actions\"><button class=\"primary mkt-update\" data-id=\""+p.id+"\" data-title=\""+esc(p.title)+"\" data-desc=\""+esc(p.description||"")+"\" data-cat=\""+esc(p.category||"")+"\">"+T("mkt_save")+"</button><button class=\"mkt-deactivate\" data-id=\""+p.id+"\">"+T("mkt_deactivate")+"</button></div></div>").join(""):"<div class=\"empty\">"+T("mkt_empty_listings")+"</div>";
listBox.querySelectorAll(".mkt-update").forEach(b=>b.onclick=async()=>{const id=b.dataset.id;const body={title:b.dataset.title,description:b.dataset.desc,category:b.dataset.cat,active:true};const priceRaw=listBox.querySelector(".mkt-up-price[data-id=\""+id+"\"]").value.trim();if(priceRaw)body.price_coins=priceRaw;const stock=listBox.querySelector(".mkt-up-stock[data-id=\""+id+"\"]").value;body.stock=stock===""?null:Number(stock);try{await post("/api/storefront/products/"+id+"/update",body);loadSeller()}catch(e){alert(T("mkt_update_fail",{message:e.message}))}});
listBox.querySelectorAll(".mkt-deactivate").forEach(b=>b.onclick=async()=>{try{await post("/api/storefront/products/"+b.dataset.id+"/deactivate",{});loadSeller()}catch(e){alert(T("mkt_deactivate_fail",{message:e.message}))}});
}catch(e){listBox.innerHTML="<div class=\"empty\">"+T("mkt_seller_unavailable",{message:esc(e.message)})+"</div>";}
}

async function loadOrders(){
const box=document.getElementById("mktOrders");
if(!box)return;
try{
const d=await get("/api/storefront/orders");
box.innerHTML=(d.orders||[]).length?(d.orders||[]).map(o=>"<div class=\"task-card\"><div class=\"task-head\"><b>#"+o.order_id+" · "+esc(o.title)+"</b><span class=\"badge badge-"+esc(o.status)+"\">"+T("status_"+o.status)+"</span></div><div class=\"task-meta\"><span>"+T("mkt_role",{role:T("mkt_"+o.role)})+"</span><span>"+coins(o.amount&&o.amount.coins)+"</span><span>"+T("mkt_fee",{amount:coins(o.fee&&o.fee.coins)})+"</span><span>"+T("mkt_buyer")+" "+esc(String(o.buyer||"").slice(0,10))+"…</span><span>"+T("mkt_seller")+" "+esc(String(o.seller||"").slice(0,10))+"…</span></div>"+(o.disputed?"<p><b>"+T("mkt_dispute")+"</b> "+esc(o.dispute_reason||"")+" "+T("mkt_awaiting_ruling")+"</p>":"")+"<div class=\"eco-actions\">"+(o.role==="buyer"&&o.status==="held"&&!o.disputed?"<button class=\"primary mkt-confirm\" data-id=\""+o.order_id+"\">"+T("mkt_confirm")+"</button><button class=\"mkt-dispute\" data-id=\""+o.order_id+"\">"+T("mkt_open_dispute")+"</button>":"")+"</div></div>").join(""):"<div class=\"empty\">"+T("mkt_empty_orders")+"</div>";
box.querySelectorAll(".mkt-confirm").forEach(b=>b.onclick=async()=>{try{const r=await post("/api/storefront/orders/"+b.dataset.id+"/confirm",{});alert(T("mkt_confirm_ok",{amount:coins(r.seller_paid&&r.seller_paid.coins)}));loadOrders()}catch(e){alert(T("mkt_confirm_fail",{message:e.message}))}});
box.querySelectorAll(".mkt-dispute").forEach(b=>b.onclick=async()=>{const reason=prompt(T("mkt_dispute_prompt"));if(!reason)return;try{await post("/api/storefront/orders/"+b.dataset.id+"/dispute",{reason});alert(T("mkt_dispute_ok"));loadOrders()}catch(e){alert(T("mkt_dispute_fail",{message:e.message}))}});
}catch(e){box.innerHTML="<div class=\"empty\">"+T("mkt_orders_unavailable",{message:esc(e.message)})+"</div>";}
}

document.querySelectorAll(".nav").forEach(n=>n.addEventListener("click",()=>{if(n.dataset.view==="storefront"){document.querySelectorAll(".nav").forEach(x=>x.classList.toggle("active",x===n));document.getElementById("pageTitle").textContent=T("mkt_page_title");window.AWEI18N.ready().then(render)}}));
})();
