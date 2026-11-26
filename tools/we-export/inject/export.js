/* Wallpaper Engine —— 视频导出扩展 v3
 *
 * 功能（插件的正确定位：找到壁纸源地址文件夹 → 导出文件夹内视频文件 → 落到影匣数据目录）：
 *   1. 多选后顶部出现工具条 →「导出视频」（壁纸引擎只能右键单个，多选没有统一入口）
 *   2. 右键单个壁纸 → 菜单里也有「导出视频到影匣…」
 *   3. 前端只负责把壁纸的原始信息交给助手；助手去翻「壁纸源地址文件夹」，
 *      里面有多少视频文件就导多少（scene / web 类不再被前端一刀切掉）
 *   4. 可选开关：导出成功后自动取消订阅（仅对创意工坊壁纸有效）
 *
 * 原理：WE 界面是 CEF + AngularJS，壁纸数据在 window.browseWallpapersCtrl.wallpapers，
 *      复制文件交给本地助手（127.0.0.1:51789），因为浏览器沙箱没有文件系统权限。
 * 所有逻辑都在 try/catch 内，任何异常都不得影响 WE 本身。
 */
(function () {
  'use strict';
  if (window.__wxExport) return;
  window.__wxExport = true;

  var ENDPOINT = 'http://127.0.0.1:51789';
  var MENU_FLAG = 'data-wx-export';
  var CARD_SEL = '.browseWallpaperImage';
  var VIDEO_RE = /\.(mp4|webm|mkv|mov|avi|m4v|wmv|flv|mpg|mpeg|ts)$/i;

  var selected = [];            // 自己维护的选择（DOM 元素）
  var anchor = null;            // shift 范围选择的起点
  var lastMenu = null;
  var unsubscribeAfter = false; // 退订开关
  var busy = false;

  /* ------------------------------ 基础工具 ------------------------------ */

  function log(msg, extra) {
    try {
      fetch(ENDPOINT + '/log', {
        method: 'POST', mode: 'no-cors',
        headers: { 'Content-Type': 'text/plain' },
        body: JSON.stringify({ msg: String(msg), extra: extra || null, t: Date.now() }),
      }).catch(function () {});
    } catch (e) {}
  }

  function getCtrl() {
    try { return window.browseWallpapersCtrl || null; } catch (e) { return null; }
  }

  function titleOf(card) {
    try {
      var el = card.querySelector('.wallpaperInfo .caption');
      if (!el) return '';
      var input = el.querySelector('input');
      if (input && input.value) return input.value.trim();
      return (el.textContent || '').trim();
    } catch (e) { return ''; }
  }

  function toast(text, ms) {
    try {
      var old = document.getElementById('wx-toast');
      if (old) old.parentNode.removeChild(old);
      var d = document.createElement('div');
      d.id = 'wx-toast';
      d.textContent = text;
      d.style.cssText = 'position:fixed;left:50%;bottom:42px;transform:translateX(-50%);z-index:2147483647;' +
        'background:rgba(24,24,28,.97);color:#f2f2f5;padding:12px 18px;border-radius:10px;font-size:13px;' +
        'border:1px solid #3a3a44;box-shadow:0 8px 28px rgba(0,0,0,.6);max-width:64vw;max-height:46vh;' +
        'overflow:auto;line-height:1.6;white-space:pre-line;text-align:left;font-family:inherit';
      document.body.appendChild(d);
      setTimeout(function () { if (d.parentNode) d.parentNode.removeChild(d); }, ms || 4200);
    } catch (e) {}
  }

  function short(s) {
    s = String(s || '');
    return s.length > 38 ? s.slice(0, 36) + '…' : s;
  }

  /* --------------------------- 卡片 ↔ 壁纸对象 --------------------------- */

  function resolveCard(card) {
    var ctrl = getCtrl();
    var list = ctrl && Array.isArray(ctrl.wallpapers) ? ctrl.wallpapers : null;
    var title = titleOf(card);
    var idx = -1;
    var m = /(\d+)\s*$/.exec(String(card.id || ''));
    if (m) idx = parseInt(m[1], 10);

    function pick(w) {
      if (!w) return null;
      var file = String(w.file || '');
      return {
        path: file,
        title: String(w.title || title || ''),
        type: String(w.type || ''),
        workshopid: w.workshopid ? String(w.workshopid) : '',
        local: !!w.local,
        // 仅用于界面提示；真正的取舍由助手按「源文件夹里有没有视频」决定
        isVideo: VIDEO_RE.test(file),
      };
    }

    if (list && idx >= 0 && list[idx]) {
      var w = list[idx];
      var wt = String(w.title || '');
      if (!wt || !title || wt === title || wt.indexOf(title) >= 0 || title.indexOf(wt) >= 0) return pick(w);
    }
    if (list && title) {
      var hits = list.filter(function (x) { return String(x.title || '') === title; });
      if (hits.length) return pick(hits[0]);
    }
    log('无法定位壁纸', { id: card.id, title: title, listLen: list ? list.length : 0 });
    return null;
  }

  /* ------------------------------ 选择管理 ------------------------------ */

  function realCard(node) {
    try {
      if (!node || !node.closest) return null;
      var c = node.closest(CARD_SEL);
      return c && !c.classList.contains('folder') ? c : null;
    } catch (e) { return null; }
  }

  function allCards() {
    try {
      return [].slice.call(document.querySelectorAll(CARD_SEL)).filter(function (c) {
        return !c.classList.contains('folder');
      });
    } catch (e) { return []; }
  }

  function onMouseDown(e) {
    try {
      var card = realCard(e.target);
      if (card) {
        if (e.ctrlKey || e.metaKey) {
          var i = selected.indexOf(card);
          if (i >= 0) selected.splice(i, 1); else selected.push(card);
          anchor = card;
        } else if (e.shiftKey && anchor) {
          var cards = allCards();
          var a = cards.indexOf(anchor), b = cards.indexOf(card);
          if (a >= 0 && b >= 0) {
            var lo = Math.min(a, b), hi = Math.max(a, b);
            selected = cards.slice(lo, hi + 1);
          } else {
            selected = [card];
          }
        } else {
          selected = [card];
          anchor = card;
        }
        renderBar();
      } else {
        // 点在空白/别的控件上：不立刻清空（避免点工具条自己就清掉了）
      }
    } catch (err) {}
  }

  function clearSelection() {
    selected = [];
    anchor = null;
    renderBar();
  }

  /* ------------------------------ 顶部工具条 ------------------------------ */

  function barEl() {
    return document.getElementById('wx-export-bar');
  }

  function renderBar() {
    try {
      var n = selected.length;
      var bar = barEl();
      if (n === 0) {
        if (bar && bar.parentNode) bar.parentNode.removeChild(bar);
        return;
      }
      if (!bar) {
        bar = document.createElement('div');
        bar.id = 'wx-export-bar';
        bar.style.cssText = 'position:fixed;top:10px;left:50%;transform:translateX(-50%);z-index:2147483600;' +
          'display:flex;align-items:center;gap:10px;padding:7px 12px;border-radius:10px;' +
          'background:rgba(22,22,26,.97);border:1px solid #3a3a44;color:#eee;font-size:13px;' +
          'box-shadow:0 10px 30px rgba(0,0,0,.55);font-family:inherit;user-select:none';
        document.body.appendChild(bar);
      }
      bar.innerHTML = '';
      var info = document.createElement('span');
      info.style.cssText = 'opacity:.9;white-space:nowrap';
      info.textContent = '已选 ' + n + ' 张';
      bar.appendChild(info);

      var btn = document.createElement('button');
      btn.textContent = busy ? '导出中…' : '导出视频';
      btn.disabled = busy;
      btn.style.cssText = 'cursor:pointer;border:none;border-radius:6px;padding:6px 14px;font-size:13px;' +
        'background:' + (busy ? '#3a3a44' : '#4f7cff') + ';color:#fff;font-family:inherit';
      btn.onclick = function (ev) { ev.stopPropagation(); doExport(); };
      bar.appendChild(btn);

      var lab = document.createElement('label');
      lab.style.cssText = 'display:flex;align-items:center;gap:5px;cursor:pointer;opacity:.92;white-space:nowrap';
      var cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.checked = unsubscribeAfter;
      cb.style.cssText = 'cursor:pointer;accent-color:#4f7cff';
      cb.onchange = function () { unsubscribeAfter = cb.checked; };
      lab.appendChild(cb);
      lab.appendChild(document.createTextNode('导出后取消订阅'));
      bar.appendChild(lab);

      var x = document.createElement('button');
      x.textContent = '✕';
      x.title = '清空选择';
      x.style.cssText = 'cursor:pointer;background:transparent;border:none;color:#8a8a96;font-size:14px;padding:0 2px';
      x.onclick = function (ev) { ev.stopPropagation(); clearSelection(); };
      bar.appendChild(x);
    } catch (e) { log('工具条渲染失败', String(e)); }
  }

  /* ------------------------------ 导出执行 ------------------------------ */

  function doExport(retry) {
    if (busy) return;
    retry = retry || 0;
    // UI 初始化有先后，ctrl 可能还没挂上；等它几拍再说
    if (!getCtrl() && retry < 8) {
      if (retry === 0) toast('正在读取壁纸列表…', 1500);
      setTimeout(function () { doExport(retry + 1); }, 300);
      return;
    }
    var cards = selected.slice();
    if (!cards.length) { toast('先选中壁纸'); return; }

    // 这里不再按扩展名过滤：scene / web 类壁纸的项目文件夹里同样可能藏着视频，
    // 有没有视频得由助手去翻「壁纸源地址文件夹」才知道。
    var items = [], unresolved = 0;
    for (var i = 0; i < cards.length; i++) {
      var r = resolveCard(cards[i]);
      if (!r) { unresolved++; continue; }
      items.push({ path: r.path, title: r.title, workshopid: r.workshopid, type: r.type });
    }

    if (!items.length) {
      toast('没能定位到选中的壁纸（' + unresolved + ' 张）');
      return;
    }
    if (unresolved) {
      toast('有 ' + unresolved + ' 张没定位到，已跳过，正在导出 ' + items.length + ' 个…', 2600);
    }

    busy = true;
    renderBar();
    log('请求导出', { items: items.length, unresolved: unresolved, unsub: unsubscribeAfter });

    postExportWithRetry(items).then(function (res) {
      busy = false;
      renderBar();
      if (!res) return; // 重试耗尽时 postExportWithRetry 已经报过错了
      if (!res || res.ok === false) {
        toast('导出失败：' + ((res && res.error) || '未知错误'));
        return;
      }
      var all = res.results || [];
      var exported = all.filter(function (x) { return x.ok && !x.skipped; });
      var skipped = all.filter(function (x) { return x.skipped; });
      var failed = all.filter(function (x) { return !x.ok; });

      var lines = [];
      if (exported.length) {
        lines.push('✓ 已导出 ' + exported.length + ' 个视频');
        exported.slice(0, 6).forEach(function (x) { lines.push('    ' + short(x.title)); });
        if (exported.length > 6) lines.push('    …还有 ' + (exported.length - 6) + ' 个');
      }
      if (skipped.length) {
        if (lines.length) lines.push('');
        lines.push('– 跳过 ' + skipped.length + ' 个');
        skipped.slice(0, 5).forEach(function (x) { lines.push('    ' + short(x.title) + '（' + (x.reason || '') + '）'); });
        if (skipped.length > 5) lines.push('    …还有 ' + (skipped.length - 5) + ' 个');
      }
      if (failed.length) {
        if (lines.length) lines.push('');
        lines.push('✗ 失败 ' + failed.length + ' 个');
        failed.slice(0, 4).forEach(function (x) { lines.push('    ' + short(x.title) + '：' + (x.error || '')); });
      }
      if (res.target) {
        lines.push('');
        lines.push('位置：' + res.target);
        var buckets = {};
        exported.forEach(function (x) { if (x.bucket) buckets[x.bucket] = (buckets[x.bucket] || 0) + 1; });
        var bk = Object.keys(buckets);
        if (bk.length) {
          lines.push('区间夹：' + bk.map(function (b) { return b + '（' + buckets[b] + ' 个）'; }).join('、'));
        }
      }
      if (res.imported) lines.push('已入库影匣：' + res.imported + ' 条');

      toast(lines.join('\n'), 11000);
      log('导出完成', { exported: exported.length, skipped: skipped.length, failed: failed.length, target: res.target });

      if (unsubscribeAfter && exported.length) {
        unsubscribeAll(exported);
      } else {
        clearSelection();
      }
    }).catch(function (e) {
      busy = false;
      renderBar();
      toast('导出中断：' + String(e && e.message || e));
      log('导出请求失败', String(e));
    });
  }

  /**
   * 带自动恢复的导出请求（2026-11-27：不再把「连不上」直接甩给用户）。
   *
   * 助手由守护进程看着，死了 2 秒内就会自动拉起；忙的时候（大导出阻塞事件循环）
   * 几十秒内必然忙完。所以这里用递增间隔重试 + 等待提示，把瞬时故障全部吃掉，
   * 用户端体会就是「导出转了一会儿，成了」—— 这才是跟壁纸引擎捆绑存活该有的样子。
   */
  function postExportWithRetry(items, attempt) {
    attempt = attempt || 1;
    toast('正在连接导出助手…（第 ' + attempt + ' 次尝试）', 2200);
    return fetch(ENDPOINT + '/export', {
      method: 'POST',
      headers: { 'Content-Type': 'text/plain' },
      body: JSON.stringify({ items: items }),
    }).then(function (r) { return r.json(); }).catch(function (e) {
      log('第 ' + attempt + ' 次请求失败，稍后重试', String(e));
      if (attempt >= 5) {
        toast('导出助手连续 5 次没有响应（127.0.0.1:51789）。\n守护进程会继续尝试自动恢复，稍等半分钟再点一次即可；\n若一直如此，请完全退出壁纸引擎后重开。');
        return null;
      }
      // 1.5s → 3s → 6s → 9s：给守护进程拉起、忙完手头导出的时间
      return new Promise(function (resolve) {
        setTimeout(resolve, Math.min(1500 * attempt, 9000));
      }).then(function () {
        return postExportWithRetry(items, attempt + 1);
      });
    });
  }

  /* ------------------------------ 取消订阅 ------------------------------ */

  function unsubscribeAll(okList) {
    var ids = okList.map(function (x) { return x.workshopid; }).filter(Boolean);
    if (!ids.length) {
      toast('导出完成。这些都是本地壁纸，没有可取消的订阅');
      clearSelection();
      return;
    }
    try {
      window.unsubscribeWorkshopItemCallback = function () {
        log('取消订阅回调', ids);
        toast('已取消订阅 ' + ids.length + ' 个壁纸');
        clearSelection();
      };
      window.unsubscribeWorkshopItemCallbackError = function (err) {
        log('取消订阅失败', String(err));
        toast('取消订阅失败：' + err);
        clearSelection();
      };
      window.host.ui.unsubscribeWorkshopItem(ids);
      log('发起取消订阅', ids);
    } catch (e) {
      log('取消订阅异常', String(e));
      toast('取消订阅出错：' + e.message);
      clearSelection();
    }
  }

  /* ------------------------------ 右键菜单 ------------------------------ */

  function buildItem() {
    var li = document.createElement('li');
    li.setAttribute(MENU_FLAG, '1');
    var a = document.createElement('a');
    a.className = 'ctxLink';
    a.style.cursor = 'pointer';
    var icon = document.createElement('span');
    icon.className = 'fas fa-file-video fa-fw ctxMenuIcon';
    var label = document.createElement('span');
    label.className = 'ctxLabel';
    label.textContent = '导出视频到影匣…';
    a.appendChild(icon);
    a.appendChild(label);
    li.appendChild(a);
    a.addEventListener('click', function (e) {
      e.preventDefault(); e.stopPropagation();
      closeMenu();
      if (selected.length > 1) {
        toast('已选 ' + selected.length + ' 张，请用顶部工具条批量导出');
      } else {
        doExport();
      }
    }, true);
    a.addEventListener('mousedown', function (e) { e.stopPropagation(); }, true);
    return li;
  }

  function injectInto(menu) {
    try {
      if (!menu || !menu.querySelector) return;
      if (menu.querySelector('[' + MENU_FLAG + ']')) return;
      if (!menu.querySelector('.ctxLink')) return;
      var item = buildItem();
      var divider = menu.querySelector('li.divider');
      if (divider) menu.insertBefore(item, divider); else menu.appendChild(item);
      lastMenu = menu;
    } catch (e) {}
  }

  function scanMenus() {
    try {
      var menus = document.querySelectorAll('ul.dropdown-menu.ctxMenu');
      for (var i = 0; i < menus.length; i++) injectInto(menus[i]);
    } catch (e) {}
  }

  function closeMenu() {
    try {
      if (lastMenu && lastMenu.parentNode) lastMenu.parentNode.removeChild(lastMenu);
      lastMenu = null;
      document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, clientX: 2, clientY: 2 }));
    } catch (e) {}
  }

  function onContextMenu(e) {
    try {
      var card = realCard(e.target);
      if (!card) return;
      if (selected.indexOf(card) < 0) { selected = [card]; anchor = card; renderBar(); }
      // 菜单是右键后同步生成的，分几次扫，避免一直挂 MutationObserver 拖慢整个界面
      setTimeout(scanMenus, 0);
      setTimeout(scanMenus, 60);
      setTimeout(scanMenus, 200);
    } catch (err) {}
  }

  /* ------------------------------ 启动 ------------------------------ */

  function boot() {
    try {
      // 全局错误捕获：既方便排查扩展自己，也能抓到 WE 的脚本异常
      window.addEventListener('error', function (e) {
        try {
          log('JS错误', {
            msg: e.message, src: String(e.filename || '').slice(-60), line: e.lineno,
            stack: String((e.error && e.error.stack) || '').slice(0, 500),
          });
        } catch (x) {}
      });
      window.addEventListener('unhandledrejection', function (e) {
        try { log('未处理Promise', String((e.reason && (e.reason.stack || e.reason.message)) || e.reason).slice(0, 400)); } catch (x) {}
      });

      document.addEventListener('mousedown', onMouseDown, true);
      document.addEventListener('contextmenu', onContextMenu, true);

      // 切页面/滚动后选择可能失效，定期校对：卡片不在文档里就从集合里剔除
      setInterval(function () {
        if (!selected.length) { if (barEl()) renderBar(); return; }
        var alive = selected.filter(function (c) { return document.body.contains(c); });
        if (alive.length !== selected.length) { selected = alive; renderBar(); }
      }, 1500);

      log('扩展已加载 v3', {
        ctrl: !!getCtrl(),
        wallpapers: (getCtrl() && getCtrl().wallpapers) ? getCtrl().wallpapers.length : 0,
        unsubApi: !!(window.host && window.host.ui && typeof window.host.ui.unsubscribeWorkshopItem === 'function'),
      });
    } catch (e) {
      log('初始化失败', String(e));
    }
  }

  if (document.readyState === 'complete' || document.readyState === 'interactive') boot();
  else document.addEventListener('DOMContentLoaded', boot);
})();
