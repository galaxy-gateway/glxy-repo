#!/usr/bin/env python3
"""Build docs/presentation/index.html from deck.json + slides/*.html.

Run:  python3 docs/presentation/build.py && open docs/presentation/index.html
Keys: arrows/space/PgUp/PgDn = navigate, Home/End, n = speaker notes, f = fullscreen.
"""
import json, pathlib
d = pathlib.Path(__file__).parent
deck = json.loads((d / "deck.json").read_text())
fonts = "&".join(f["href"].split("css2?")[1].replace("&display=swap", "") for f in deck["faces"].values() if "href" in f)
slides = "\n".join((d / "slides" / f"{i}.html").read_text() for i in deck["order"])
html = f"""<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{deck["title"]}</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?{fonts}&display=swap">
<style>
html,body{{margin:0;height:100%;background:#050709;overflow:hidden}}
#stage{{position:absolute;left:0;top:0;width:1920px;height:1080px;transform-origin:0 0}}
section{{position:relative;width:1920px;height:1080px;box-sizing:border-box;overflow:hidden}}
section:not(.show){{display:none!important}}
h1,h2,h3,p{{margin:0}} h1{{font-size:96px;line-height:1.1}} h2{{font-size:64px;line-height:1.15}} h3{{font-size:44px;line-height:1.2}} p{{font-size:32px;line-height:1.4}}
h1,h2,h3{{font-weight:600}} table{{border-collapse:collapse;width:100%}} td,th{{padding:.35em .6em;border-bottom:1px solid #8886}} th{{font-weight:600}}
aside{{display:none}} #notes{{position:fixed;left:0;right:0;bottom:0;padding:16px 24px;background:#000d;color:#eee;font:20px/1.4 sans-serif;display:none}}
[data-build-in]{{opacity:0;transition:opacity .5s,transform .5s}} .on [data-build-in]{{opacity:1;transform:none}}
.on [data-build-in^="rise"]{{transform:none}} section:not(.on) [data-build-in^="rise"]{{transform:translateY(40px)}}
</style></head><body><div id="stage">
{slides}
</div><div id="notes"></div>
<script>
const S=[...document.querySelectorAll('section')],st=document.getElementById('stage'),nt=document.getElementById('notes');
let i=Math.max(0,Math.min(S.length-1,(parseInt(location.hash.slice(1))||1)-1)),showNotes=false;
S.forEach(s=>{{s.querySelectorAll('x-connector').forEach(c=>{{const g=n=>+c.getAttribute(n),col=c.style.color||'#999',w=parseFloat(c.style.borderWidth)||2,ns='http://www.w3.org/2000/svg';
const v=document.createElementNS(ns,'svg');v.setAttribute('width',1920);v.setAttribute('height',1080);v.style.cssText='position:absolute;left:0;top:0;pointer-events:none';
const x1=g('x1'),y1=g('y1'),x2=g('x2'),y2=g('y2'),a=Math.atan2(y2-y1,x2-x1),h=w*4;
v.innerHTML=`<line x1="${{x1}}" y1="${{y1}}" x2="${{x2-Math.cos(a)*h/2}}" y2="${{y2-Math.sin(a)*h/2}}" stroke="${{col}}" stroke-width="${{w}}"/><polygon fill="${{col}}" points="${{x2}},${{y2}} ${{x2-h*Math.cos(a-.4)}},${{y2-h*Math.sin(a-.4)}} ${{x2-h*Math.cos(a+.4)}},${{y2-h*Math.sin(a+.4)}}"/>`;
const b=c.getAttribute('data-build-in');if(b)v.setAttribute('data-build-in',b);c.replaceWith(v)}});
s.querySelectorAll('[data-build-in]').forEach(e=>{{const o=parseInt(e.dataset.buildIn.split(' ')[1])||1;e.style.transitionDelay=(o*.35)+'s'}})}});
function fit(){{const k=Math.min(innerWidth/1920,innerHeight/1080);st.style.transform=`scale(${{k}})`;st.style.left=(innerWidth-1920*k)/2+'px';st.style.top=(innerHeight-1080*k)/2+'px'}}
function go(n){{i=Math.max(0,Math.min(S.length-1,n));S.forEach(s=>s.classList.remove('on','show'));S[i].classList.add('show');void S[i].offsetWidth;S[i].classList.add('on');
history.replaceState(null,'','#'+(i+1));const a=S[i].querySelector('aside');nt.textContent=a?a.textContent:'';nt.style.display=showNotes?'block':'none'}}
addEventListener('resize',fit);addEventListener('keydown',e=>{{const k=e.key;
if(['ArrowRight','ArrowDown','PageDown',' '].includes(k))go(i+1);else if(['ArrowLeft','ArrowUp','PageUp'].includes(k))go(i-1);else if(k=='Home')go(0);else if(k=='End')go(S.length-1);
else if(k=='n'){{showNotes=!showNotes;go(i)}}else if(k=='f')document.fullscreenElement?document.exitFullscreen():document.documentElement.requestFullscreen()}});
addEventListener('click',e=>go(e.clientX>innerWidth/2?i+1:i-1));fit();go(i);
</script></body></html>"""
(d / "index.html").write_text(html + "\n")
print("wrote", d / "index.html", len(deck["order"]), "slides")
