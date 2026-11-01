'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const {test} = require('node:test');

const root = path.resolve(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'ui/index.html'), 'utf8');
const source = fs.readFileSync(path.join(root, 'ui/app.js'), 'utf8');

function interfaceFixture() {
  const elements = new Map([...html.matchAll(/\bid="([^"]+)"/g)].map(([, id]) => [id, {
    innerHTML:'', textContent:'', value:id === 'library-sort' ? 'recency' : '',
    listeners:{}, addEventListener(name, listener){this.listeners[name] = listener;},
  }]));
  const context = vm.createContext({
    document:{
      getElementById(id){assert.ok(elements.has(id), `Unknown element ${id}`); return elements.get(id);},
      querySelector(selector){return selector.startsWith('meta') ? {content:'test-token'} : {addEventListener(){}};},
      querySelectorAll(){return [];}, addEventListener(){},
    },
    window:{addEventListener(){}},
    // Keep startup pending; each test supplies state directly to the real renderer.
    fetch(){return new Promise(() => {});},
    FormData:class FormData {}, setTimeout(){}, clearTimeout(){}, setInterval(){},
  });
  vm.runInContext(source, context);
  const row = (id, added, points, rating, count) => ({
    clip:{id, title:id, extension:'mp4', duration_secs:3, width:100, height:100, size:1000},
    seeding:true, added_at_ms:added, points_earned:points, average_rating:rating, rating_count:count,
  });
  context.fixture = [
    row('popular', 200, 30, 3, 10), row('quality', 100, 4, 8, 1),
    row('new', 400, 0, null, 0), row('tied', 300, 4, 8, 2), row('legacy', 0, 0, null, 0),
  ];
  vm.runInContext('state={library:fixture};', context);
  return {context, elements, render(){vm.runInContext('renderLibrary();', context);}};
}

function order(element) {
  return [...element.innerHTML.matchAll(/data-preview="([^"]+)"/g)].map(([, id]) => id);
}

test('library sorts independently by recency, points and average rating', () => {
  const app = interfaceFixture();
  const original = JSON.stringify(app.context.fixture);
  for (const [sort, expected] of [
    ['recency', ['new', 'tied', 'popular', 'quality', 'legacy']],
    ['points', ['popular', 'tied', 'quality', 'new', 'legacy']],
    ['rating', ['tied', 'quality', 'popular', 'new', 'legacy']],
  ]) {
    app.elements.get('library-sort').value = sort;
    app.elements.get('library-sort').listeners.change();
    assert.deepEqual(order(app.elements.get('library')), expected);
  }
  assert.equal(JSON.stringify(app.context.fixture), original, 'sorting must not reorder API state');
});

test('rating ties and unknown legacy dates have stable content-ID ordering', () => {
  const app = interfaceFixture();
  app.context.fixture[1].added_at_ms = 300;
  app.context.fixture.reverse();
  app.elements.get('library-sort').value = 'rating';
  app.render();
  assert.deepEqual(order(app.elements.get('library')), ['quality', 'tied', 'popular', 'new', 'legacy']);
  for (const row of app.context.fixture) row.added_at_ms = 0;
  app.elements.get('library-sort').value = 'recency';
  app.render();
  assert.deepEqual(order(app.elements.get('library')), ['legacy', 'new', 'popular', 'quality', 'tied']);
});

test('refresh keeps the selected sort and updates scores without confusing unrated clips', () => {
  const app = interfaceFixture();
  app.elements.get('library-sort').value = 'rating';
  app.render();
  assert.match(app.elements.get('library').innerHTML, /8\.0 \/ 9<\/strong> · 1 rating</);
  assert.match(app.elements.get('library').innerHTML, /8\.0 \/ 9<\/strong> · 2 ratings</);
  assert.match(app.elements.get('library').innerHTML, /Unrated/);
  assert.match(app.elements.get('library').innerHTML, /30\.00<\/strong> points earned/);
  app.context.fixture[2].average_rating = 9;
  app.context.fixture[2].rating_count = 1;
  app.render();
  assert.equal(order(app.elements.get('library'))[0], 'new');
  assert.equal(app.elements.get('library-sort').value, 'rating');
  assert.doesNotMatch(app.elements.get('library').innerHTML, /NaN|undefined/);
});

test('empty library renders and titles stay escaped', () => {
  const app = interfaceFixture();
  app.context.fixture[0].clip.title = '<img src=x onerror=alert(1)>';
  app.render();
  assert.match(app.elements.get('library').innerHTML, /&lt;img/);
  assert.doesNotMatch(app.elements.get('library').innerHTML, /<img/);
  vm.runInContext('state.library=[];', app.context);
  app.render();
  assert.match(app.elements.get('library').innerHTML, /No clips yet/);
});
