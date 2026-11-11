#!/usr/bin/env python3
"""Build docs/presentation/index.html from deck.json + slides/*.html.

Run:  python3 docs/presentation/build.py && open docs/presentation/index.html
Keys: arrows/space/PgUp/PgDn = navigate, Home/End, n = speaker notes, f = fullscreen.

The viewer mirrors the claude.ai Slides features the deck uses: fade, push and
magic transitions (magic moves pinned elements that share an id across two
slides), data-build-in kinds, straight x-connector arrows, and x-embed live
pages (rendered as sandboxed iframes). It honours prefers-reduced-motion.
"""
import html, json, pathlib, re

d = pathlib.Path(__file__).parent
deck = json.loads((d / "deck.json").read_text())
fonts = "&".join(f["href"].split("css2?")[1].replace("&display=swap", "") for f in deck["faces"].values() if "href" in f)
slides = "\n".join((d / "slides" / f"{i}.html").read_text() for i in deck["order"])
slides = re.sub(
    r'<x-embed style="([^"]*)">(.*?)</x-embed>',
    lambda m: f'<iframe sandbox="allow-scripts" style="{m[1]}" srcdoc="{html.escape(m[2])}"></iframe>',
    slides,
    flags=re.S,
)

PAGE = r"""<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>__TITLE__</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?__FONTS__&display=swap">
<style>
html,body{margin:0;height:100%;background:#050709;overflow:hidden}
#stage{position:absolute;left:0;top:0;width:1920px;height:1080px;transform-origin:0 0;overflow:hidden}
section{position:absolute;left:0;top:0;width:1920px;height:1080px;box-sizing:border-box;overflow:hidden}
section *{box-sizing:border-box} section:not(.show):not(.out){display:none!important} section.show{z-index:1}
h1,h2,h3,p,ul{margin:0} h1{font-size:96px;line-height:1.1} h2{font-size:64px;line-height:1.15} h3{font-size:44px;line-height:1.2} p{font-size:32px;line-height:1.4}
h1,h2,h3{font-weight:600} table{border-collapse:collapse;width:100%} td,th{padding:.35em .6em;border-bottom:1px solid #8886} th{font-weight:600}
iframe{border:0} aside{display:none}
#notes{position:fixed;left:0;right:0;bottom:0;padding:16px 24px;background:#000d;color:#eee;font:20px/1.4 sans-serif;display:none;z-index:9}
#bar{position:fixed;left:0;bottom:0;height:4px;background:#5ad1b8;transition:width .3s;z-index:9}
[data-build-in]{opacity:0;transition:opacity .5s,translate .5s cubic-bezier(.2,.8,.2,1),scale .5s cubic-bezier(.3,1.5,.5,1)}
[data-build-in^=rise]{translate:0 40px} [data-build-in^=drop]{translate:0 -40px} [data-build-in^=left]{translate:-60px 0}
[data-build-in^=right]{translate:60px 0} [data-build-in^=scale]{scale:.8} [data-build-in^=pop]{scale:.4}
.on [data-build-in],.mm{opacity:1;translate:none;scale:none} .mm{transition:none!important}
@media(prefers-reduced-motion:reduce){[data-build-in],#bar{transition:none}}
</style></head><body><div id="stage">
__SLIDES__
</div><div id="notes"></div><div id="bar"></div>
<script>
const S=[...document.querySelectorAll('section')],st=document.getElementById('stage'),nt=document.getElementById('notes'),bar=document.getElementById('bar'),
RM=matchMedia('(prefers-reduced-motion:reduce)').matches,px=(e,p)=>parseFloat(e.style[p]),EASE='cubic-bezier(.3,0,.2,1)';
let i=Math.max(0,Math.min(S.length-1,(parseInt(location.hash.slice(1))||1)-1)),showNotes=false,tick=0,hid=[];
S.forEach(s=>{s.querySelectorAll('x-connector').forEach(c=>{const g=n=>+c.getAttribute(n),col=c.style.color||'#999',w=parseFloat(c.style.borderWidth)||2,ns='http://www.w3.org/2000/svg';
const v=document.createElementNS(ns,'svg');v.setAttribute('width',1920);v.setAttribute('height',1080);v.style.cssText='position:absolute;left:0;top:0;pointer-events:none';
const x1=g('x1'),y1=g('y1'),x2=g('x2'),y2=g('y2'),a=Math.atan2(y2-y1,x2-x1),h=w*4;
v.innerHTML=`<line x1="${x1}" y1="${y1}" x2="${x2-Math.cos(a)*h/2}" y2="${y2-Math.sin(a)*h/2}" stroke="${col}" stroke-width="${w}"/><polygon fill="${col}" points="${x2},${y2} ${x2-h*Math.cos(a-.4)},${y2-h*Math.sin(a-.4)} ${x2-h*Math.cos(a+.4)},${y2-h*Math.sin(a+.4)}"/>`;
const b=c.getAttribute('data-build-in');if(b)v.setAttribute('data-build-in',b);c.replaceWith(v)});
s.querySelectorAll('[data-build-in]').forEach(e=>{const o=parseInt(e.dataset.buildIn.split(' ')[1])||1;e.style.transitionDelay=(o*.3)+'s'})});
function fit(){const k=Math.min(innerWidth/1920,innerHeight/1080);st.style.transform=`scale(${k})`;st.style.left=(innerWidth-1920*k)/2+'px';st.style.top=(innerHeight-1080*k)/2+'px'}
// Magic move: a pinned element with the same id on both slides glides from its old box to its new one.
function magic(a,b){b.querySelectorAll(':scope>[id]').forEach(n=>{const o=a.querySelector(':scope>#'+CSS.escape(n.id));if(!o)return;
const w=px(n,'width'),h=px(n,'height'),ow=px(o,'width'),oh=px(o,'height'),sz=w&&h&&ow&&oh,
dx=px(o,'left')+(ow||0)/2-px(n,'left')-(w||0)/2,dy=px(o,'top')+(sz?oh/2:0)-px(n,'top')-(sz?h/2:0);
n.classList.add('mm');o.style.visibility='hidden';hid.push(o);
n.animate([{translate:dx+'px '+dy+'px',scale:sz?ow/w:1,transform:o.style.transform||'none'},{translate:'0 0',scale:1,transform:n.style.transform||'none'}],{duration:900,easing:EASE})})}
function go(n,instant){n=Math.max(0,Math.min(S.length-1,n));const a=S[i],b=S[n],fwd=n>=i,t=++tick,tr=instant||RM||a===b?'':((fwd?a:b).dataset.transition||'fade');i=n;
document.querySelectorAll('.mm').forEach(e=>e.classList.remove('mm'));hid.forEach(e=>e.style.visibility='');hid=[];
S.forEach(s=>s.classList.remove('on','show','out'));b.querySelectorAll('iframe').forEach(f=>f.srcdoc=f.srcdoc);b.classList.add('show');
if(tr){a.classList.add('out');
if(tr=='push'){const w=fwd?1920:-1920;b.animate([{translate:w+'px 0'},{translate:'0 0'}],{duration:600,easing:EASE});a.animate([{translate:'0 0'},{translate:-w+'px 0'}],{duration:600,easing:EASE})}
else if(tr=='magic'){magic(a,b);[...b.children].forEach(c=>{if(!c.classList.contains('mm')&&!c.hasAttribute('data-build-in'))c.animate([{opacity:0},{opacity:1}],{duration:600})})}
else b.animate([{opacity:0},{opacity:1}],{duration:500});
setTimeout(()=>{if(t===tick)a.classList.remove('out')},950)}
void b.offsetWidth;b.classList.add('on');
history.replaceState(null,'','#'+(i+1));const s=b.querySelector('aside');nt.textContent=s?s.textContent:'';nt.style.display=showNotes?'block':'none';bar.style.width=((i+1)/S.length*100)+'%'}
addEventListener('resize',fit);addEventListener('keydown',e=>{const k=e.key;
if(['ArrowRight','ArrowDown','PageDown',' '].includes(k))go(i+1);else if(['ArrowLeft','ArrowUp','PageUp'].includes(k))go(i-1);else if(k=='Home')go(0);else if(k=='End')go(S.length-1);
else if(k=='n'){showNotes=!showNotes;go(i,1)}else if(k=='f')document.fullscreenElement?document.exitFullscreen():document.documentElement.requestFullscreen()});
addEventListener('click',e=>go(e.clientX>innerWidth/2?i+1:i-1));fit();go(i,1);
</script></body></html>"""

out = PAGE.replace("__TITLE__", html.escape(deck["title"])).replace("__FONTS__", fonts).replace("__SLIDES__", slides)
(d / "index.html").write_text(out + "\n")
print("wrote", d / "index.html", len(deck["order"]), "slides")
