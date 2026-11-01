from pathlib import Path
p = Path('ui/index.html')
s = p.read_text()
start = s.index('<body>')
end = s.index('<section id="page-rate"')
s = s[:start] + '''<body data-page="home">
<div class="app">
<header class="app-header">
<a class="brand" href="#home" aria-label="Clipcrop main menu"><span class="brandmark" aria-hidden="true">c<span>c</span></span>clipcrop</a>
<div class="points" aria-label="Points balance"><span class="balance-label">POINTS</span><span class="balance" id="balance">100<span>.00</span></span></div>
</header>
<main>
<div class="screen-bar"><button id="back-home" class="back-button" data-goto="home" aria-label="Back to main menu"><span aria-hidden="true">←</span> Menu</button><span id="breadcrumb" class="visually-hidden">Home</span></div>
<section id="page-home" class="page home-page" aria-labelledby="menu-title">
<h1 id="menu-title">Main menu</h1>
<nav class="main-menu" aria-label="Main menu">
<button data-page="seed" aria-label="Upload"><svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 16V4m-4 4 4-4 4 4M5 15v5h14v-5"/></svg><span class="menu-label">Upload</span><span class="nav-count" id="seed-count" title="Clips in your library">0</span><span class="menu-arrow" aria-hidden="true">→</span></button>
<button data-page="rate" aria-label="Rate"><svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="m10 8 6 4-6 4Z"/></svg><span class="menu-label">Rate</span><span class="nav-count" id="ready-count" title="Clips ready to rate">0</span><span class="menu-arrow" aria-hidden="true">→</span></button>
<button data-page="roll" aria-label="Roll"><svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="3"/><path d="M8 8h.01M16 8h.01M12 12h.01M8 16h.01M16 16h.01" stroke-width="2.5"/></svg><span class="menu-label">Roll</span><span class="menu-arrow" aria-hidden="true">→</span></button>
<button data-page="settings" aria-label="Settings"><svg class="nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 7h3m4 0h9M4 17h9m4 0h3"/><circle cx="9" cy="7" r="2"/><circle cx="15" cy="17" r="2"/></svg><span class="menu-label">Settings</span><span class="menu-arrow" aria-hidden="true">→</span></button>
</nav>
</section>
''' + s[end:]
s = s.replace('<section id="page-rate" class="page">', '<section id="page-rate" class="page" hidden>')
s = s.replace('</main></div>', '''</main>
<footer class="app-footer"><div class="peer-status" role="status"><span class="status-dot" aria-hidden="true"></span><span id="peer-status">Finding peers…</span></div><span id="profile-name" title="Your display name">Anonymous peer</span></footer>
</div>''')
s = s.replace('<h1>Rate clips</h1>', '<h1 tabindex="-1">Rate clips</h1>').replace('<h1>Your clips</h1>', '<h1 tabindex="-1">Your clips</h1>').replace('<h1>Roll</h1>', '<h1 tabindex="-1">Roll</h1>').replace('<h1>Settings</h1>', '<h1 tabindex="-1">Settings</h1>')
p.write_text(s)
p = Path('README.md')
s = p.read_text().replace('## The four screens\n', '## The four screens\n\nThe app opens in a compact, resizable window with four menu rows: Upload, Rate, Roll, and Settings. Points stay in the top-right corner, with peer status and your display name in the bottom corners. Use **← Menu** or Escape to return to the main menu. Rating sessions start when you enter Rate and end when you leave it.\n')
p.write_text(s)
