'use strict';

(function () {
  var ACTIVE_PHASES = {
    checking: true,
    downloading: true,
    verifying: true,
    cleaning: true,
    url_downloading: true,
    uploading: true,
    restarting: true
  };

  var PHASE_META = {
    idle: { label: '待检查', tone: 'neutral' },
    checking: { label: '检查中', tone: 'busy' },
    downloading: { label: '下载中', tone: 'busy' },
    verifying: { label: '校验中', tone: 'busy' },
    cleaning: { label: '清理旧版', tone: 'busy' },
    url_downloading: { label: 'URL 下载中', tone: 'busy' },
    uploading: { label: '上传中', tone: 'busy' },
    restarting: { label: '重启中', tone: 'busy' },
    up_to_date: { label: '已是最新', tone: 'ok' },
    update_available: { label: '发现更新', tone: 'update' },
    success: { label: '部署成功', tone: 'ok' },
    error: { label: '异常', tone: 'error' },
    disabled: { label: '已停用', tone: 'disabled' }
  };

  var CATEGORY_LABEL = {
    core: '服务端核心',
    plugin: '插件',
    mod: '模组',
    other: '其他'
  };

  var HTML_ESCAPES = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;'
  };

  var store = {
    system: null,
    tasks: [],
    config: null,
    meta: null,
    filter: 'all',
    pending: new Map(),
    pendingCheckAll: false,
    checkAllSeenBusy: false,
    checkAllStarted: 0,
    pendingBatch: false,
    batchSeenActive: false,
    batchStarted: 0,
    processTaskId: null,
    restarts: [],
    statusLoading: false,
    statusErrorNotified: false,
    refreshTimer: null,
    snackTimer: null,
    snackAction: null,
    logCollapsed: false,
    seenLogSeq: new Set(),
    seenLogQueue: [],
    logCount: 0,
    logWasConnected: false,
    confirmResolver: null,
    source: null
  };

  var els = {};

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }

  function init() {
    cacheElements();
    bindEvents();
    restoreLogState();
    connectLogs();
    refreshStatus();
    loadConfig();
    window.setInterval(refreshStatus, 2500);
    window.setInterval(tickClocks, 1000);
    window.setTimeout(function () {
      refreshStatus();
    }, 600);
  }

  function cacheElements() {
    els.appShell = document.getElementById('appShell');
    els.systemChip = document.getElementById('systemChip');
    els.systemChipLabel = document.getElementById('systemChipLabel');
    els.checkAllButton = document.getElementById('checkAllButton');
    els.batchButton = document.getElementById('batchButton');
    els.settingsButton = document.getElementById('settingsButton');
    els.restartBadge = document.getElementById('restartBadge');
    els.restartBadgeText = document.getElementById('restartBadgeText');
    els.overviewSub = document.getElementById('overviewSub');
    els.metricUpdates = document.getElementById('metricUpdates');
    els.metricBusy = document.getElementById('metricBusy');
    els.metricQuota = document.getElementById('metricQuota');
    els.metricQuotaLabel = document.getElementById('metricQuotaLabel');
    els.metricNext = document.getElementById('metricNext');
    els.taskCountBadge = document.getElementById('taskCountBadge');
    els.taskGrid = document.getElementById('taskGrid');
    els.addTaskButton = document.getElementById('addTaskButton');
    els.logDock = document.getElementById('logDock');
    els.logBody = document.getElementById('logBody');
    els.logLive = document.getElementById('logLive');
    els.logCount = document.getElementById('logCount');
    els.clearLogsButton = document.getElementById('clearLogsButton');
    els.toggleLogsButton = document.getElementById('toggleLogsButton');
    els.logToggleIcon = document.getElementById('logToggleIcon');
    els.snackbar = document.getElementById('snackbar');
    els.snackbarText = document.getElementById('snackbarText');
    els.snackbarAction = document.getElementById('snackbarAction');
    els.snackbarClose = document.getElementById('snackbarClose');
    els.processDialog = document.getElementById('processDialog');
    els.processSubtitle = document.getElementById('processSubtitle');
    els.processCurrent = document.getElementById('processCurrent');
    els.processCurrentText = document.getElementById('processCurrentText');
    els.processRetryChip = document.getElementById('processRetryChip');
    els.processMeta = document.getElementById('processMeta');
    els.processTimeline = document.getElementById('processTimeline');
    els.taskDialog = document.getElementById('taskDialog');
    els.taskDialogTitle = document.getElementById('taskDialogTitle');
    els.taskForm = document.getElementById('taskForm');
    els.taskFormError = document.getElementById('taskFormError');
    els.taskFormErrorText = document.getElementById('taskFormErrorText');
    els.taskName = document.getElementById('taskName');
    els.taskSourceType = document.getElementById('taskSourceType');
    els.taskSourceLabel = document.getElementById('taskSourceLabel');
    els.taskSourceHint = document.getElementById('taskSourceHint');
    els.taskSourceError = document.getElementById('taskSourceError');
    els.taskRepo = document.getElementById('taskRepo');
    els.taskRegex = document.getElementById('taskRegex');
    els.taskGameVersions = document.getElementById('taskGameVersions');
    els.taskLoaders = document.getElementById('taskLoaders');
    els.taskPlatformFilters = document.getElementById('field-task-platform-filters');
    els.taskCleanupOld = document.getElementById('taskCleanupOld');
    els.taskCleanupRegex = document.getElementById('taskCleanupRegex');
    els.taskDaemon = document.getElementById('taskDaemon');
    els.taskInstance = document.getElementById('taskInstance');
    els.taskTarget = document.getElementById('taskTarget');
    els.taskRestart = document.getElementById('taskRestart');
    els.taskPrerelease = document.getElementById('taskPrerelease');
    els.taskPrereleaseRow = document.getElementById('taskPrereleaseRow');
    els.taskEnabled = document.getElementById('taskEnabled');
    els.taskDeleteButton = document.getElementById('taskDeleteButton');
    els.taskSubmitButton = document.getElementById('taskSubmitButton');
    els.settingsDialog = document.getElementById('settingsDialog');
    els.settingsForm = document.getElementById('settingsForm');
    els.settingsFormError = document.getElementById('settingsFormError');
    els.settingsFormErrorText = document.getElementById('settingsFormErrorText');
    els.settingsInterval = document.getElementById('settingsInterval');
    els.settingsBaseUrl = document.getElementById('settingsBaseUrl');
    els.settingsApiKey = document.getElementById('settingsApiKey');
    els.apiKeyHint = document.getElementById('apiKeyHint');
    els.settingsDaemonScheme = document.getElementById('settingsDaemonScheme');
    els.settingsTimeout = document.getElementById('settingsTimeout');
    els.settingsApiInterval = document.getElementById('settingsApiInterval');
    els.settingsApiRetries = document.getElementById('settingsApiRetries');
    els.settingsUrlThreshold = document.getElementById('settingsUrlThreshold');
    els.settingsRestartMode = document.getElementById('settingsRestartMode');
    els.settingsInsecure = document.getElementById('settingsInsecure');
    els.settingsGithubToken = document.getElementById('settingsGithubToken');
    els.githubTokenHint = document.getElementById('githubTokenHint');
    els.settingsGithubApi = document.getElementById('settingsGithubApi');
    els.settingsCurseforgeKey = document.getElementById('settingsCurseforgeKey');
    els.curseforgeKeyHint = document.getElementById('curseforgeKeyHint');
    els.settingsPath = document.getElementById('settingsPath');
    els.settingsSubmitButton = document.getElementById('settingsSubmitButton');
    els.batchDialog = document.getElementById('batchDialog');
    els.batchForm = document.getElementById('batchForm');
    els.batchFormError = document.getElementById('batchFormError');
    els.batchFormErrorText = document.getElementById('batchFormErrorText');
    els.batchTaskList = document.getElementById('batchTaskList');
    els.batchSelectAll = document.getElementById('batchSelectAll');
    els.batchSelectedCount = document.getElementById('batchSelectedCount');
    els.batchRestartAt = document.getElementById('batchRestartAt');
    els.batchRestartMode = document.getElementById('batchRestartMode');
    els.batchSubmitButton = document.getElementById('batchSubmitButton');
    els.restartList = document.getElementById('restartList');
    els.confirmDialog = document.getElementById('confirmDialog');
    els.confirmTitle = document.getElementById('confirmTitle');
    els.confirmMessage = document.getElementById('confirmMessage');
    els.confirmAccept = document.getElementById('confirmAccept');
    els.confirmCancel = document.getElementById('confirmCancel');
    els.filterButtons = Array.prototype.slice.call(document.querySelectorAll('.filter-chip'));
  }

  function bindEvents() {
    els.checkAllButton.addEventListener('click', triggerCheckAll);
    els.batchButton.addEventListener('click', openBatchDialog);
    els.restartBadge.addEventListener('click', openBatchDialog);
    els.settingsButton.addEventListener('click', openSettings);
    els.systemChip.addEventListener('click', openSettings);
    els.addTaskButton.addEventListener('click', function () { openTaskDialog(null); });
    els.taskSourceType.addEventListener('change', updateSourceUi);
    els.batchForm.addEventListener('submit', onBatchSubmit);
    els.batchSelectAll.addEventListener('change', toggleAllBatchTasks);
    els.batchTaskList.addEventListener('change', updateBatchSelection);
    els.restartList.addEventListener('click', onRestartListClick);

    els.filterButtons.forEach(function (button) {
      button.addEventListener('click', function () {
        store.filter = button.getAttribute('data-filter') || 'all';
        els.filterButtons.forEach(function (item) {
          item.setAttribute('aria-pressed', item === button ? 'true' : 'false');
        });
        renderTasks();
      });
    });

    els.taskGrid.addEventListener('click', onTaskGridClick);
    els.taskForm.addEventListener('submit', onTaskSubmit);
    els.settingsForm.addEventListener('submit', onSettingsSubmit);
    els.taskDeleteButton.addEventListener('click', onTaskDelete);

    Array.prototype.forEach.call(document.querySelectorAll('[data-close-dialog]'), function (button) {
      button.addEventListener('click', function () {
        var id = button.getAttribute('data-close-dialog');
        var dialog = document.getElementById(id);
        if (dialog) {
          dialog.close();
        }
      });
    });

    Array.prototype.forEach.call(document.querySelectorAll('dialog.m3-dialog'), function (dialog) {
      dialog.addEventListener('click', function (event) {
        if (event.target === dialog) {
          dialog.close();
        }
      });
    });

    els.clearLogsButton.addEventListener('click', clearLogs);
    els.toggleLogsButton.addEventListener('click', toggleLogs);
    els.snackbarClose.addEventListener('click', hideSnackbar);
    els.snackbarAction.addEventListener('click', function () {
      var action = store.snackAction;
      hideSnackbar();
      if (typeof action === 'function') {
        action();
      }
    });

    els.confirmAccept.addEventListener('click', function () { resolveConfirm(true); });
    els.confirmCancel.addEventListener('click', function () { resolveConfirm(false); });
    els.confirmDialog.addEventListener('close', function () {
      if (store.confirmResolver) {
        var resolver = store.confirmResolver;
        store.confirmResolver = null;
        resolver(false);
      }
    });

    window.addEventListener('resize', syncLogMetrics);
    window.addEventListener('online', function () { refreshStatus(); });
    document.addEventListener('visibilitychange', function () {
      if (!document.hidden) {
        refreshStatus();
      }
    });
  }

  function esc(value) {
    return String(value === undefined || value === null ? '' : value).replace(/[&<>"']/g, function (character) {
      return HTML_ESCAPES[character];
    });
  }

  function icon(name, extraClass) {
    return '<svg class="icon ' + (extraClass || '') + '" aria-hidden="true"><use href="#i-' + name + '"></use></svg>';
  }

  async function api(path, options) {
    var opts = options || {};
    var headers = Object.assign({ 'Accept': 'application/json' }, opts.headers || {});
    if (opts.body) {
      headers['Content-Type'] = 'application/json';
    }
    var response = await fetch(path, Object.assign({}, opts, { headers: headers }));
    var text = await response.text();
    var data = null;
    if (text) {
      try {
        data = JSON.parse(text);
      } catch (error) {
        data = { error: text.slice(0, 300) };
      }
    }
    if (!response.ok) {
      var message = data && data.error ? data.error : '请求失败 (HTTP ' + response.status + ')';
      throw new Error(message);
    }
    return data;
  }

  function scheduleRefresh(delay) {
    window.clearTimeout(store.refreshTimer);
    store.refreshTimer = window.setTimeout(function () {
      refreshStatus();
    }, delay);
  }

  async function refreshStatus() {
    if (store.statusLoading) {
      return;
    }
    store.statusLoading = true;
    try {
      var data = await api('/api/status');
      store.system = data.system;
      store.tasks = data.tasks || [];
      store.restarts = data.scheduled_restarts || [];
      store.statusErrorNotified = false;
      reconcilePending();
      renderSystem();
      renderTasks();
      if (els.processDialog.open) {
        renderProcessDialog();
      }
      renderRestartBadge();
      if (els.batchDialog.open) {
        renderRestartList();
      }
      updateTopButton();
      updateBatchButton();
      if (store.pending.size > 0 || store.pendingCheckAll || store.pendingBatch) {
        scheduleRefresh(1200);
      }
    } catch (error) {
      handleStatusError(error);
    } finally {
      store.statusLoading = false;
    }
  }

  function handleStatusError(error) {
    if (!store.tasks.length) {
      renderFatalError(error.message);
    }
    els.systemChip.classList.remove('is-busy');
    els.systemChip.classList.add('is-warning');
    els.systemChipLabel.textContent = '连接中断';
    els.overviewSub.textContent = '无法连接后台 API：' + error.message;
    if (!store.statusErrorNotified) {
      store.statusErrorNotified = true;
      showSnackbar('无法获取系统状态：' + error.message, '重试', function () { refreshStatus(); });
    }
  }

  function reconcilePending() {
    if (!store.system) {
      return;
    }
    store.pending.forEach(function (pending, id) {
      var task = findTask(id);
      if (!task) {
        store.pending.delete(id);
        return;
      }
      if (ACTIVE_PHASES[task.phase]) {
        pending.seenBusy = true;
        return;
      }
      var elapsed = Date.now() - pending.started;
      if (pending.seenBusy || elapsed > 1500) {
        store.pending.delete(id);
        if (task.phase === 'error') {
          showSnackbar('「' + task.name + '」执行失败：' + (task.last_error || '未知错误'), '查看日志', openLogs);
        } else {
          showSnackbar('「' + task.name + '」' + phaseLabel(task), '查看日志', openLogs);
        }
      }
    });

    if (store.pendingCheckAll) {
      if (store.system.checking_all) {
        store.checkAllSeenBusy = true;
      } else if (store.checkAllSeenBusy || Date.now() - store.checkAllStarted > 1500) {
        store.pendingCheckAll = false;
        store.checkAllSeenBusy = false;
        var updateCount = store.system.update_count || 0;
        showSnackbar('全局检查完成，发现 ' + updateCount + ' 个可更新任务', '查看日志', openLogs);
      }
    }

    if (store.pendingBatch) {
      if (store.system.batch_active) {
        store.batchSeenActive = true;
      } else if (store.batchSeenActive || Date.now() - store.batchStarted > 2000) {
        store.pendingBatch = false;
        store.batchSeenActive = false;
        showSnackbar('批量部署结束，可在「批量部署」中查看统一重启计划', '打开计划', openBatchDialog);
      }
    }
  }

  function renderSystem() {
    var system = store.system;
    if (!system) {
      return;
    }
    var quota = null;
    store.tasks.forEach(function (task) {
      if (typeof task.rate_limit_remaining === 'number') {
        if (quota === null || task.rate_limit_remaining < quota) {
          quota = task.rate_limit_remaining;
        }
      }
    });

    if (system.batch_active) {
      els.systemChip.classList.remove('is-warning');
      els.systemChip.classList.add('is-busy');
      els.systemChipLabel.textContent = '批量部署中';
    } else if (system.checking_all) {
      els.systemChip.classList.remove('is-warning');
      els.systemChip.classList.add('is-busy');
      els.systemChipLabel.textContent = '全局检查中';
    } else if (!system.mcsm_configured) {
      els.systemChip.classList.remove('is-busy');
      els.systemChip.classList.add('is-warning');
      els.systemChipLabel.textContent = 'MCSM 待配置';
    } else {
      els.systemChip.classList.remove('is-warning', 'is-busy');
      els.systemChipLabel.textContent = '服务运行中';
    }

    els.metricUpdates.textContent = String(system.update_count || 0) + ' / ' + String(system.enabled_count || 0);
    els.metricBusy.textContent = String(system.busy_count || 0);
    els.metricQuota.textContent = quota === null ? '—' : quota.toLocaleString('en-US');
    els.metricQuotaLabel.textContent = system.github_authenticated ? 'GitHub 剩余 · Token' : 'GitHub 剩余额度';

    var bits = [];
    bits.push('已启用 ' + String(system.enabled_count || 0) + ' 个更新通道');
    bits.push(system.mcsm_configured ? 'MCSM 已配置' : '等待配置 MCSM');
    bits.push('运行 ' + formatUptime(system.uptime_secs));
    if (system.last_check_at) {
      bits.push('上次检查 ' + formatRelative(system.last_check_at));
    }
    if (system.scheduled_restart_count) {
      bits.push(system.scheduled_restart_count + ' 个统一重启待执行');
    }
    els.overviewSub.textContent = bits.join(' · ');
    els.taskCountBadge.textContent = String((store.tasks || []).length);
    var dense = (store.tasks || []).length > 3;
    if (els.appShell.classList.contains('is-dense') !== dense) {
      els.appShell.classList.toggle('is-dense', dense);
      syncLogMetrics();
    }
    tickClocks();
  }

  function tickClocks() {
    if (!store.system) {
      return;
    }
    if (store.system.checking_all) {
      els.metricNext.textContent = '检查中';
      return;
    }
    var next = store.system.next_check_at;
    if (!next) {
      els.metricNext.textContent = '等待调度';
      return;
    }
    var remaining = Math.max(0, Math.ceil((next - Date.now()) / 1000));
    els.metricNext.textContent = formatCountdown(remaining);
  }

  function renderTasks() {
    var tasks = filterTasks();
    var signature = [store.filter, JSON.stringify(store.tasks), Array.from(store.pending.keys()).join(',')].join('|');
    if (els.taskGrid.getAttribute('data-signature') === signature) {
      return;
    }
    els.taskGrid.setAttribute('data-signature', signature);
    els.taskGrid.setAttribute('aria-busy', 'false');

    if (!store.tasks.length) {
      els.taskGrid.innerHTML = emptyStateHtml('empty', '还没有更新通道', '创建一个任务，把 GitHub、Modrinth、CurseForge 或 Spigot 资产自动送到 MCSManager 实例。', '新增第一个任务', 'add');
      return;
    }
    if (!tasks.length) {
      els.taskGrid.innerHTML = emptyStateHtml('filter', '没有符合条件的任务', '当前筛选条件下暂无任务，试试切换筛选器。', '显示全部', 'reset-filter');
      return;
    }

    els.taskGrid.innerHTML = tasks.map(function (task) {
      return taskCardHtml(task);
    }).join('');
  }

  function filterTasks() {
    return store.tasks.filter(function (task) {
      if (store.filter === 'updates') {
        return Boolean(task.update_available);
      }
      if (store.filter === 'busy') {
        return isTaskBusy(task);
      }
      if (store.filter === 'error') {
        return task.phase === 'error';
      }
      return true;
    });
  }

  function taskCardHtml(task) {
    var category = task.category || 'other';
    if (!CATEGORY_LABEL[category]) {
      category = 'other';
    }
    var categoryLabel = task.category_label || CATEGORY_LABEL[category];
    var busy = isTaskBusy(task);
    var phase = task.phase || 'idle';
    var meta = statusMeta(task, busy);
    var iconName = category === 'core' ? 'server' : (category === 'plugin' ? 'plug' : (category === 'mod' ? 'layers' : 'box'));
    var currentVersion = task.current_version ? esc(task.current_version) : '未部署';
    var latestVersion = task.latest_version ? esc(task.latest_version) : '—';
    var currentIsEmpty = !task.current_version;
    var latestIsEmpty = !task.latest_version;
    var assetName = task.latest_asset ? esc(task.latest_asset) : '等待首次来源检查';
    var assetLabel = busy && task.current_detail ? esc(task.current_detail) : assetName;
    var footnote = task.current_version && task.last_updated_at
      ? '部署于 ' + formatRelative(task.last_updated_at)
      : (task.last_checked_at ? '检查于 ' + formatRelative(task.last_checked_at) : '尚未检查');
    if (task.restart_deferred && phase === 'success') {
      footnote = '待统一重启 · ' + footnote;
    }
    var provider = task.source_type || 'github';
    var providerLabel = platformLabel(provider);
    var sourceValue = String(task.source || '');
    var sourceHref = providerSourceUrl(provider, sourceValue);
    var sourceIconName = provider === 'github' ? 'github' : (provider === 'spigot' ? 'plug' : 'package');
    var actionDisabled = busy || !task.enabled ? ' disabled' : '';
    var progress = progressHtml(task);
    var error = '';
    if (phase === 'error' && task.last_error) {
      error = '<div class="card-error" title="' + esc(task.last_error) + '">' + icon('alert') + '<span>' + esc(task.last_error) + '</span></div>';
    }
    var spinner = busy ? '<span class="loading loading-spinner" style="width:14px;height:14px;border-width:2px"></span>' : icon('download', 'icon-sm');
    var statusDot = ACTIVE_PHASES[phase] || busy
      ? '<span class="loading loading-spinner" style="width:9px;height:9px;border-width:1.5px"></span>'
      : '<span class="status-dot" aria-hidden="true"></span>';

    return '<article class="task-card" data-id="' + esc(task.id) + '" data-phase="' + esc(phase) + '" data-disabled="' + (task.enabled ? 'false' : 'true') + '">'
      + '<div class="task-top">'
      +   '<div class="type-emblem type-' + category + '" aria-hidden="true">' + icon(iconName) + '</div>'
      +   '<div class="task-title-block">'
      +     '<div class="task-title-row"><h3 class="task-name" title="' + esc(task.name) + '">' + esc(task.name) + '</h3>'
      +       '<span class="type-chip type-' + category + '">' + esc(categoryLabel) + '</span>'
      +       '<span class="type-chip type-other">' + esc(providerLabel) + '</span></div>'
      +     '<a class="repo-link" href="' + esc(sourceHref) + '" target="_blank" rel="noopener noreferrer" title="打开 ' + esc(providerLabel) + ' 来源">'
      +       icon(sourceIconName, 'icon-sm') + '<span class="truncate">' + esc(sourceValue) + '</span>' + icon('external', 'icon-sm') + '</a>'
      +   '</div>'
      +   '<span class="status-chip tone-' + esc(meta.tone) + '">' + statusDot + '<span>' + esc(meta.label) + '</span></span>'
      + '</div>'
      + '<div class="task-meta">'
      +   '<span title="实例 UUID">' + icon('server', 'icon-sm') + '<span class="truncate">' + esc(shortId(task.mcsm_instance_uuid)) + '</span></span>'
      +   '<span title="' + esc(task.mcsm_target_path) + '">' + icon('folder', 'icon-sm') + '<span class="truncate">' + esc(task.mcsm_target_path) + '</span></span>'
      + '</div>'
      + '<div class="version-panel">'
      +   '<div class="version-cell"><span class="version-label">当前版本</span><span class="version-value ' + (currentIsEmpty ? 'is-empty' : '') + '" title="' + currentVersion + '">' + currentVersion + '</span></div>'
      +   '<div class="version-arrow" aria-hidden="true">' + icon('arrow-right', 'icon-sm') + '</div>'
      +   '<div class="version-cell"><span class="version-label">最新版本</span><span class="version-value ' + (latestIsEmpty ? 'is-empty' : 'is-latest') + '" title="' + latestVersion + '">' + latestVersion + '</span></div>'
      + '</div>'
      + pipelineHtml(task)
      + '<div class="asset-row" title="' + esc(task.current_detail || task.latest_asset || '') + '">' + icon(busy ? 'activity' : 'package', 'icon-sm') + '<code>' + assetLabel + '</code></div>'
      + progress
      + error
      + '<div class="card-footer">'
      +   '<span class="card-footnote">' + (task.enabled ? footnote : '通道已停用') + '</span>'
      +   '<div class="card-actions">'
      +     '<button type="button" class="icon-btn" data-action="process" data-id="' + esc(task.id) + '" title="查看部署过程" aria-label="查看部署过程">' + icon('activity', 'icon-sm') + '</button>'
      +     '<button type="button" class="icon-btn" data-action="check" data-id="' + esc(task.id) + '" title="立即检查" aria-label="立即检查"' + actionDisabled + '>' + icon('refresh', 'icon-sm') + '</button>'
      +     '<button type="button" class="m3-btn m3-btn-filled" data-action="update" data-id="' + esc(task.id) + '"' + actionDisabled + '>' + spinner + '<span>' + (busy && !ACTIVE_PHASES[phase] ? '处理中' : '立即更新') + '</span></button>'
      +     '<button type="button" class="m3-btn m3-btn-text" data-action="edit" data-id="' + esc(task.id) + '">编辑</button>'
      +   '</div>'
      + '</div>'
      + '</article>';
  }

  function pipelineHtml(task) {
    var labels = task.restart ? ['检查', '下载', '上传', '重启'] : ['检查', '下载', '上传', '完成'];
    var progress = pipelineProgress(task);
    var html = '<div class="pipeline" aria-label="部署管线">';
    for (var index = 0; index < labels.length; index += 1) {
      var classes = 'pipeline-step';
      if (index < progress.done) {
        classes += ' is-done';
      }
      if (index === progress.active) {
        classes += ' is-active';
      }
      html += '<div class="' + classes + '"><span class="step-dot" aria-hidden="true"></span><span class="step-label">' + labels[index] + '</span></div>';
    }
    return html + '</div>';
  }

  function pipelineProgress(task) {
    var phase = task.phase || 'idle';
    if (phase === 'checking') {
      return { done: 0, active: 0 };
    }
    if (phase === 'downloading' || phase === 'verifying') {
      return { done: 1, active: 1 };
    }
    if (phase === 'cleaning' || phase === 'url_downloading' || phase === 'uploading') {
      return { done: 2, active: 2 };
    }
    if (phase === 'restarting') {
      return { done: 3, active: 3 };
    }
    if (phase === 'success') {
      return { done: 4, active: -1 };
    }
    if (phase === 'up_to_date' || phase === 'update_available') {
      return { done: 1, active: -1 };
    }
    return { done: 0, active: -1 };
  }

  function progressHtml(task) {
    var phase = task.phase;
    var progress = task.progress;
    if (phase === 'cleaning') {
      return '<div class="card-progress"><div class="progress-meta"><span>清理目标目录中的旧版文件</span><span>清理中</span></div><div class="linear-progress is-indeterminate"><span></span></div></div>';
    }
    if (phase === 'downloading') {
      var percent = progress && typeof progress.percent === 'number' ? progress.percent : 0;
      var total = progress && progress.total ? formatBytes(progress.total) : '未知大小';
      var downloaded = progress ? formatBytes(progress.downloaded) : '0 B';
      var indeterminate = !progress || !progress.total;
      return '<div class="card-progress">'
        + '<div class="progress-meta"><span>下载资产 · ' + esc(downloaded) + ' / ' + esc(total) + '</span><span>' + (indeterminate ? '进行中' : Math.round(percent) + '%') + '</span></div>'
        + '<div class="linear-progress' + (indeterminate ? ' is-indeterminate' : '') + '"><span style="width:' + Math.max(0, Math.min(100, percent)) + '%"></span></div>'
        + '</div>';
    }
    if (phase === 'url_downloading') {
      var urlPercent = progress && typeof progress.percent === 'number' ? progress.percent : 0;
      var urlTotal = progress && progress.total ? formatBytes(progress.total) : '未知大小';
      var urlDownloaded = progress ? formatBytes(progress.downloaded) : '0 B';
      var urlIndeterminate = !progress || !progress.total;
      return '<div class="card-progress">'
        + '<div class="progress-meta"><span>Daemon 正在从来源下载 · ' + esc(urlDownloaded) + ' / ' + esc(urlTotal) + '</span><span>' + (urlIndeterminate ? '进行中' : Math.round(urlPercent) + '%') + '</span></div>'
        + '<div class="linear-progress' + (urlIndeterminate ? ' is-indeterminate' : '') + '"><span style="width:' + Math.max(0, Math.min(100, urlPercent)) + '%"></span></div>'
        + '</div>';
    }
    if (phase === 'uploading') {
      return '<div class="card-progress"><div class="progress-meta"><span>正在直传 MCSM Daemon</span><span>上传中</span></div><div class="linear-progress is-indeterminate"><span></span></div></div>';
    }
    if (phase === 'restarting') {
      return '<div class="card-progress"><div class="progress-meta"><span>文件已就位，正在重启实例</span><span>重启中</span></div><div class="linear-progress is-indeterminate"><span></span></div></div>';
    }
    if (phase === 'verifying') {
      return '<div class="card-progress"><div class="progress-meta"><span>校验 SHA-256 完整性</span><span>校验中</span></div><div class="linear-progress is-indeterminate"><span></span></div></div>';
    }
    return '';
  }

  function statusMeta(task, busy) {
    if (!task.enabled) {
      return { label: '已停用', tone: 'disabled' };
    }
    if (task.restart_deferred && task.phase === 'success') {
      return { label: '待统一重启', tone: 'update' };
    }
    if (busy && task.retry_count > 0) {
      return { label: '重试中', tone: 'update' };
    }
    if (busy && !ACTIVE_PHASES[task.phase]) {
      return { label: '处理中', tone: 'busy' };
    }
    return PHASE_META[task.phase] || PHASE_META.idle;
  }

  function phaseLabel(task) {
    if (task.phase === 'error') {
      return '执行失败';
    }
    if (task.phase === 'success') {
      return '部署完成';
    }
    if (task.phase === 'up_to_date') {
      return '检查完成，已是最新版本';
    }
    if (task.phase === 'update_available') {
      return '检查完成，发现新版本';
    }
    var meta = PHASE_META[task.phase];
    return meta ? meta.label : '操作完成';
  }

  function isTaskBusy(task) {
    return Boolean(ACTIVE_PHASES[task.phase]) || store.pending.has(task.id);
  }

  function findTask(id) {
    for (var index = 0; index < store.tasks.length; index += 1) {
      if (store.tasks[index].id === id) {
        return store.tasks[index];
      }
    }
    return null;
  }

  function emptyStateHtml(kind, title, copy, actionLabel, action) {
    var iconName = kind === 'filter' ? 'package' : 'rocket';
    var button = action
      ? '<button type="button" class="m3-btn m3-btn-tonal" data-action="' + action + '">' + (action === 'add' ? icon('plus', 'icon-sm') : '') + actionLabel + '</button>'
      : '';
    return '<div class="empty-state">'
      + '<div class="empty-icon" aria-hidden="true">' + icon(iconName) + '</div>'
      + '<h3 class="empty-title">' + esc(title) + '</h3>'
      + '<p class="empty-copy">' + esc(copy) + '</p>'
      + button
      + '</div>';
  }

  function renderFatalError(message) {
    els.taskGrid.setAttribute('data-signature', '');
    els.taskGrid.setAttribute('aria-busy', 'false');
    els.taskGrid.innerHTML = '<div class="empty-state">'
      + '<div class="empty-icon" style="color:var(--md-error);background:var(--md-error-container)">' + icon('alert') + '</div>'
      + '<h3 class="empty-title">后台服务不可达</h3>'
      + '<p class="empty-copy">' + esc(message) + '</p>'
      + '<button type="button" class="m3-btn m3-btn-tonal" data-action="retry">重试连接</button>'
      + '</div>';
  }

  function onTaskGridClick(event) {
    var trigger = event.target.closest('[data-action]');
    if (!trigger) {
      return;
    }
    var action = trigger.getAttribute('data-action');
    var id = trigger.getAttribute('data-id');
    if (action === 'add') {
      openTaskDialog(null);
      return;
    }
    if (action === 'reset-filter') {
      store.filter = 'all';
      els.filterButtons.forEach(function (button) {
        button.setAttribute('aria-pressed', button.getAttribute('data-filter') === 'all' ? 'true' : 'false');
      });
      renderTasks();
      return;
    }
    if (action === 'retry') {
      refreshStatus();
      return;
    }
    if (action === 'edit') {
      openTaskDialog(id);
      return;
    }
    if (action === 'process') {
      openProcessDialog(id);
      return;
    }
    if (action === 'check' || action === 'update') {
      triggerTask(id, action);
    }
  }


  function openProcessDialog(id) {
    var task = findTask(id);
    if (!task) {
      return;
    }
    store.processTaskId = id;
    renderProcessDialog();
    els.processDialog.showModal();
  }

  function renderProcessDialog() {
    var task = findTask(store.processTaskId);
    if (!task) {
      return;
    }
    els.processSubtitle.textContent = task.name + ' · ' + (task.mcsm_target_path || '未配置目标路径');
    els.processCurrentText.textContent = task.current_detail
      || (task.last_error ? '最近错误：' + task.last_error : '等待开始部署');
    var retries = Number(task.retry_count) || 0;
    if (retries > 0) {
      els.processRetryChip.style.display = '';
      els.processRetryChip.textContent = retries + ' 次冷却重试';
    } else {
      els.processRetryChip.style.display = 'none';
    }

    var meta = [];
    meta.push('<span>阶段：' + esc(statusMeta(task, isTaskBusy(task)).label) + '</span>');
    if (task.current_version || task.latest_version) {
      meta.push('<span>版本：' + esc(task.current_version || '未部署') + ' → ' + esc(task.latest_version || '—') + '</span>');
    }
    if (task.last_updated_at) {
      meta.push('<span>上次部署：' + esc(formatRelative(task.last_updated_at)) + '</span>');
    }
    if (task.restart_deferred) {
      meta.push('<span>重启已交由统一队列</span>');
    }
    els.processMeta.innerHTML = meta.join('');

    var steps = task.steps || [];
    if (!steps.length) {
      els.processTimeline.innerHTML = '<li class="process-empty">尚未执行部署。点击「立即更新」后，这里会显示内部步骤。</li>';
      return;
    }
    var html = '';
    for (var index = 0; index < steps.length; index += 1) {
      var step = steps[index];
      var status = step.status || 'pending';
      var marker;
      if (status === 'active') {
        marker = '<span class="loading loading-spinner" style="width:13px;height:13px;border-width:1.5px"></span>';
      } else {
        var markerIcon = status === 'done' ? 'check' : (status === 'failed' ? 'alert' : (status === 'skipped' ? 'info' : 'clock'));
        marker = icon(markerIcon, 'icon-sm');
      }
      var started = step.started_at ? formatTime(step.started_at) : '';
      var finished = step.finished_at ? formatTime(step.finished_at) : '';
      var metaLine = '';
      if (started) {
        metaLine = started + (finished ? ' → ' + finished : ' · 进行中');
      }
      if (step.started_at && step.finished_at) {
        var duration = Math.max(0, step.finished_at - step.started_at);
        metaLine += ' · ' + (duration < 1000 ? '&lt;1s' : Math.round(duration / 1000) + 's');
      }
      var attempts = Number(step.attempts) || 0;
      html += '<li class="process-step is-' + esc(status) + '">'
        + '<span class="process-marker">' + marker + '</span>'
        + '<div>'
        +   '<div class="process-step-title"><span>' + esc(step.label || step.key) + '</span>'
        +     (attempts > 0 ? '<span class="process-attempts">重试 ' + attempts + ' 次</span>' : '')
        +   '</div>'
        +   '<p class="process-step-detail">' + esc(step.detail || (status === 'pending' ? '等待执行' : '')) + '</p>'
        +   (metaLine ? '<div class="process-step-meta">' + metaLine + '</div>' : '')
        + '</div>'
        + '</li>';
    }
    els.processTimeline.innerHTML = html;
  }

  async function triggerTask(id, action) {
    var task = findTask(id);
    if (!task || isTaskBusy(task)) {
      return;
    }
    try {
      var result = await api('/api/tasks/' + encodeURIComponent(id) + '/' + action, { method: 'POST' });
      store.pending.set(id, { started: Date.now(), seenBusy: false, action: action });
      showSnackbar(result.message || '操作已开始', '查看日志', openLogs);
      renderTasks();
      scheduleRefresh(350);
    } catch (error) {
      showSnackbar(error.message, '重试', function () { triggerTask(id, action); });
      refreshStatus();
    }
  }

  async function triggerCheckAll() {
    if (store.pendingCheckAll || (store.system && store.system.checking_all)) {
      return;
    }
    setButtonLoading(els.checkAllButton, true);
    try {
      var result = await api('/api/check-all', { method: 'POST' });
      store.pendingCheckAll = true;
      store.checkAllSeenBusy = false;
      store.checkAllStarted = Date.now();
      els.systemChip.classList.add('is-busy');
      els.systemChip.classList.remove('is-warning');
      els.systemChipLabel.textContent = '全局检查中';
      showSnackbar(result.message || '全局检查已开始', '查看日志', openLogs);
      scheduleRefresh(400);
    } catch (error) {
      setButtonLoading(els.checkAllButton, false);
      showSnackbar(error.message, '关闭', null);
    }
    updateTopButton();
  }

  function updateTopButton() {
    var busy = store.pendingCheckAll || Boolean(store.system && store.system.checking_all);
    els.checkAllButton.classList.toggle('is-loading', busy);
    els.checkAllButton.disabled = busy;
  }

  function updateBatchButton() {
    var busy = store.pendingBatch || Boolean(store.system && store.system.batch_active);
    els.batchButton.classList.toggle('is-loading', busy);
    els.batchButton.disabled = busy;
    els.restartBadge.classList.toggle('is-loading', busy);
  }

  function renderRestartBadge() {
    var count = store.system && typeof store.system.scheduled_restart_count === 'number'
      ? store.system.scheduled_restart_count
      : (store.restarts || []).filter(function (item) { return item.status === 'pending'; }).length;
    if (count > 0) {
      els.restartBadge.style.display = 'inline-flex';
      els.restartBadgeText.textContent = count + ' 个待重启';
    } else {
      els.restartBadge.style.display = 'none';
    }
  }

  function setButtonLoading(button, loading) {
    if (!button) {
      return;
    }
    button.classList.toggle('is-loading', loading);
    button.disabled = loading;
  }

  function providerSourceUrl(provider, source) {
    if (provider === 'modrinth') {
      return 'https://modrinth.com/project/' + encodeURIComponent(source);
    }
    if (provider === 'curseforge') {
      return 'https://www.curseforge.com/projects/' + encodeURIComponent(source);
    }
    if (provider === 'spigot') {
      return 'https://www.spigotmc.org/resources/' + encodeURIComponent(source) + '/';
    }
    var parts = String(source || '').split('/');
    return 'https://github.com/' + encodeURIComponent(parts[0] || '') + '/' + encodeURIComponent(parts[1] || '');
  }

  function shortId(value) {
    var text = String(value || '');
    if (text.length <= 14) {
      return text || '实例未配置';
    }
    return text.slice(0, 8) + '…' + text.slice(-4);
  }

  function formatTime(value) {
    if (!value) {
      return '—';
    }
    var date = new Date(value);
    if (Number.isNaN(date.getTime())) {
      return '—';
    }
    return date.toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function formatRelative(value) {
    if (!value) {
      return '—';
    }
    var delta = Math.max(0, Date.now() - value);
    if (delta < 10000) {
      return '刚刚';
    }
    if (delta < 60000) {
      return Math.floor(delta / 1000) + ' 秒前';
    }
    if (delta < 3600000) {
      return Math.floor(delta / 60000) + ' 分钟前';
    }
    if (delta < 86400000) {
      return Math.floor(delta / 3600000) + ' 小时前';
    }
    var date = new Date(value);
    return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' });
  }

  function formatUptime(seconds) {
    var value = Math.max(0, Number(seconds) || 0);
    var days = Math.floor(value / 86400);
    var hours = Math.floor((value % 86400) / 3600);
    var minutes = Math.floor((value % 3600) / 60);
    if (days > 0) {
      return days + ' 天 ' + hours + ' 小时';
    }
    if (hours > 0) {
      return hours + ' 小时 ' + minutes + ' 分';
    }
    return minutes + ' 分';
  }

  function formatCountdown(seconds) {
    var value = Math.max(0, Number(seconds) || 0);
    var hours = Math.floor(value / 3600);
    var minutes = Math.floor((value % 3600) / 60);
    var secs = value % 60;
    var mm = String(minutes).padStart(2, '0');
    var ss = String(secs).padStart(2, '0');
    if (hours > 0) {
      return String(hours) + ':' + mm + ':' + ss;
    }
    return mm + ':' + ss;
  }

  function formatBytes(value) {
    var bytes = Number(value) || 0;
    if (bytes < 1024) {
      return bytes + ' B';
    }
    if (bytes < 1024 * 1024) {
      return (bytes / 1024).toFixed(1) + ' KB';
    }
    if (bytes < 1024 * 1024 * 1024) {
      return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    }
    return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
  }

  function platformLabel(type) {
    return {
      github: 'GitHub',
      modrinth: 'Modrinth',
      curseforge: 'CurseForge',
      spigot: 'SpigotMC'
    }[type] || 'GitHub';
  }

  function formatDateTime(value) {
    if (!value) {
      return '—';
    }
    var date = new Date(value);
    if (Number.isNaN(date.getTime())) {
      return '—';
    }
    return date.toLocaleString('zh-CN', {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false
    });
  }

  function localDateTimeValue(value) {
    var date = new Date(value);
    if (Number.isNaN(date.getTime())) {
      return '';
    }
    var year = date.getFullYear();
    var month = String(date.getMonth() + 1).padStart(2, '0');
    var day = String(date.getDate()).padStart(2, '0');
    var hour = String(date.getHours()).padStart(2, '0');
    var minute = String(date.getMinutes()).padStart(2, '0');
    return year + '-' + month + '-' + day + 'T' + hour + ':' + minute;
  }

  async function openBatchDialog() {
    if (!store.config) {
      await loadConfig();
    }
    if (!store.config) {
      showSnackbar('配置尚未加载完成，请稍后重试', '关闭', null);
      return;
    }
    clearFormError(els.batchFormError);
    setButtonLoading(els.batchSubmitButton, false);
    renderBatchTasks();
    var defaultAt = new Date(Date.now() + 15 * 60000);
    defaultAt.setSeconds(0, 0);
    defaultAt.setMinutes(Math.ceil(defaultAt.getMinutes() / 5) * 5);
    els.batchRestartAt.value = localDateTimeValue(defaultAt.getTime());
    renderRestartList();
    els.batchDialog.showModal();
  }

  function renderBatchTasks() {
    var tasks = (store.config.tasks || []).filter(function (task) { return task.enabled !== false; });
    if (!tasks.length) {
      els.batchTaskList.innerHTML = '<div class="batch-empty">没有已启用的任务</div>';
      els.batchSelectAll.checked = false;
      updateBatchSelection();
      return;
    }
    els.batchTaskList.innerHTML = tasks.map(function (task) {
      return '<label class="batch-task">'
        + '<input type="checkbox" value="' + esc(task.id) + '" checked>'
        + '<span class="batch-task-main"><span class="batch-task-name">' + esc(task.name) + '</span>'
        + '<span class="batch-task-meta">' + esc(shortId(task.mcsm_instance_uuid)) + ' · ' + esc(task.mcsm_target_path) + '</span></span>'
        + '<span class="batch-task-provider">' + esc(platformLabel(task.source_type)) + '</span>'
        + '</label>';
    }).join('');
    els.batchSelectAll.checked = true;
    updateBatchSelection();
  }

  function selectedBatchIds() {
    return Array.prototype.slice.call(els.batchTaskList.querySelectorAll('input[type="checkbox"]:checked')).map(function (input) {
      return input.value;
    });
  }

  function updateBatchSelection() {
    var all = Array.prototype.slice.call(els.batchTaskList.querySelectorAll('input[type="checkbox"]'));
    var selected = selectedBatchIds();
    els.batchSelectedCount.textContent = selected.length + ' 个任务';
    els.batchSelectAll.checked = all.length > 0 && selected.length === all.length;
    els.batchSelectAll.indeterminate = selected.length > 0 && selected.length < all.length;
  }

  function toggleAllBatchTasks() {
    var checked = els.batchSelectAll.checked;
    Array.prototype.forEach.call(els.batchTaskList.querySelectorAll('input[type="checkbox"]'), function (input) {
      input.checked = checked;
    });
    updateBatchSelection();
  }

  async function onBatchSubmit(event) {
    event.preventDefault();
    clearFormError(els.batchFormError);
    var ids = selectedBatchIds();
    if (!ids.length) {
      showFormError(els.batchFormError, els.batchFormErrorText, '请至少选择一个任务');
      return;
    }
    var restartAt = els.batchRestartAt.value ? new Date(els.batchRestartAt.value).getTime() : NaN;
    var restartAtOk = Number.isFinite(restartAt) && restartAt > Date.now() + 5000;
    markField('field-batch-restart-at', !restartAtOk);
    if (!restartAtOk) {
      showFormError(els.batchFormError, els.batchFormErrorText, '请选择一个至少 5 秒之后的统一重启时间');
      return;
    }
    setButtonLoading(els.batchSubmitButton, true);
    try {
      var result = await api('/api/batch/update', {
        method: 'POST',
        body: JSON.stringify({
          task_ids: ids,
          restart_at: restartAt,
          restart_mode: els.batchRestartMode.value
        })
      });
      store.pendingBatch = true;
      store.batchSeenActive = false;
      store.batchStarted = Date.now();
      setButtonLoading(els.batchSubmitButton, false);
      els.batchDialog.close();
      showSnackbar(result.message || '批量部署已开始', '查看日志', openLogs);
      scheduleRefresh(400);
      updateBatchButton();
    } catch (error) {
      setButtonLoading(els.batchSubmitButton, false);
      showFormError(els.batchFormError, els.batchFormErrorText, error.message);
    }
  }

  function renderRestartList() {
    var items = (store.restarts || []).slice().sort(function (left, right) {
      var leftPending = left.status === 'pending' ? 0 : 1;
      var rightPending = right.status === 'pending' ? 0 : 1;
      if (leftPending !== rightPending) return leftPending - rightPending;
      return left.restart_at - right.restart_at;
    }).slice(0, 8);
    if (!items.length) {
      els.restartList.innerHTML = '<div class="scheduled-empty">暂无待执行的重启计划</div>';
      return;
    }
    var statusLabels = {
      pending: '待执行',
      running: '执行中',
      done: '已完成',
      failed: '失败',
      cancelled: '已取消'
    };
    els.restartList.innerHTML = items.map(function (restart) {
      var failed = restart.status === 'failed';
      var pending = restart.status === 'pending';
      var subtitle = formatDateTime(restart.restart_at) + ' · ' + (restart.restart_mode === 'stop_then_open' ? '停止后启动' : '直接重启');
      if (restart.last_error) {
        subtitle += ' · ' + restart.last_error;
      }
      return '<div class="scheduled-item' + (failed ? ' is-failed' : '') + '">'
        + '<span class="scheduled-icon" aria-hidden="true">' + icon(pending ? 'clock' : (failed ? 'alert' : 'check')) + '</span>'
        + '<span class="scheduled-main"><span class="scheduled-title">' + esc(restart.instance_label || shortId(restart.instance_uuid)) + ' · ' + esc(statusLabels[restart.status] || restart.status) + '</span>'
        + '<span class="scheduled-meta" title="' + esc(subtitle) + '">' + esc(subtitle) + '</span></span>'
        + (pending ? '<button type="button" class="m3-btn m3-btn-text compact" data-cancel-restart="' + esc(restart.id) + '">取消</button>' : '')
        + '</div>';
    }).join('');
  }

  async function onRestartListClick(event) {
    var button = event.target.closest('[data-cancel-restart]');
    if (!button) {
      return;
    }
    var id = button.getAttribute('data-cancel-restart');
    var restart = (store.restarts || []).find(function (item) { return item.id === id; });
    var confirmed = await showConfirm('取消统一重启', '将取消 ' + (restart ? (restart.instance_label || shortId(restart.instance_uuid)) : '该实例') + ' 的计划重启，但不会回滚已部署的文件。', '取消计划');
    if (!confirmed) {
      return;
    }
    try {
      await api('/api/restarts/' + encodeURIComponent(id), { method: 'DELETE' });
      await refreshStatus();
      renderRestartList();
      showSnackbar('已取消统一重启', '关闭', null);
    } catch (error) {
      showSnackbar(error.message, '关闭', null);
    }
  }

  async function loadConfig() {
    try {
      var response = await api('/api/config');
      store.config = response.config;
      store.meta = response.meta;
      if (store.config && store.config.tasks) {
        els.taskCountBadge.textContent = String(store.config.tasks.length);
      }
      if (els.settingsDialog.open) {
        populateSettings();
      }
    } catch (error) {
      showSnackbar('配置加载失败：' + error.message, '重试', function () { loadConfig(); });
    }
  }

  async function openTaskDialog(id) {
    if (!store.config) {
      await loadConfig();
    }
    if (!store.config) {
      showSnackbar('配置尚未加载完成，请稍后重试', '关闭', null);
      return;
    }
    resetTaskForm();
    var task = id ? findConfigTask(id) : null;
    if (task) {
      els.taskDialogTitle.textContent = '编辑更新任务';
      els.taskForm.setAttribute('data-edit-id', task.id);
      els.taskName.value = task.name || '';
      els.taskSourceType.value = task.source_type || 'github';
      els.taskRepo.value = task.source || task.github_repo || '';
      els.taskRegex.value = task.asset_regex || '';
      els.taskGameVersions.value = (task.game_versions || []).join(', ');
      els.taskLoaders.value = (task.loaders || []).join(', ');
      els.taskCleanupOld.checked = Boolean(task.cleanup_old);
      els.taskCleanupRegex.value = task.cleanup_regex || '';
      els.taskDaemon.value = task.mcsm_daemon_id || '';
      els.taskInstance.value = task.mcsm_instance_uuid || '';
      els.taskTarget.value = task.mcsm_target_path || '';
      els.taskRestart.checked = task.restart !== false;
      els.taskPrerelease.checked = Boolean(task.include_prerelease);
      els.taskEnabled.checked = task.enabled !== false;
      setCategory(task.category || 'auto');
      els.taskDeleteButton.style.display = 'inline-flex';
    } else {
      els.taskDialogTitle.textContent = '新增更新任务';
      els.taskForm.setAttribute('data-edit-id', '');
      els.taskSourceType.value = 'github';
      els.taskRestart.checked = true;
      els.taskPrerelease.checked = false;
      els.taskEnabled.checked = true;
      els.taskCleanupOld.checked = false;
      setCategory('auto');
      els.taskDeleteButton.style.display = 'none';
    }
    updateSourceUi();
    clearFormError(els.taskFormError);
    els.taskDialog.showModal();
    window.setTimeout(function () { els.taskName.focus(); }, 80);
  }

  function findConfigTask(id) {
    if (!store.config || !store.config.tasks) {
      return null;
    }
    for (var index = 0; index < store.config.tasks.length; index += 1) {
      if (store.config.tasks[index].id === id) {
        return store.config.tasks[index];
      }
    }
    return null;
  }

  function resetTaskForm() {
    els.taskForm.reset();
    setCategory('auto');
    updateSourceUi();
    Array.prototype.forEach.call(els.taskForm.querySelectorAll('.field'), function (field) {
      field.classList.remove('has-error');
    });
    clearFormError(els.taskFormError);
    setButtonLoading(els.taskSubmitButton, false);
    els.taskDeleteButton.disabled = false;
  }

  function setCategory(value) {
    var radio = document.querySelector('input[name="taskCategory"][value="' + value + '"]');
    if (!radio) {
      radio = document.getElementById('cat-auto');
    }
    if (radio) {
      radio.checked = true;
    }
  }

  function getCategory() {
    var checked = document.querySelector('input[name="taskCategory"]:checked');
    return checked ? checked.value : 'auto';
  }

  function updateSourceUi() {
    var type = els.taskSourceType.value || 'github';
    var isGithub = type === 'github';
    var isModrinth = type === 'modrinth';
    var isCurseforge = type === 'curseforge';
    var isSpigot = type === 'spigot';
    var labels = {
      github: ['GitHub 仓库', 'owner/repository', '例如 EssentialsX/Essentials'],
      modrinth: ['Modrinth 项目', 'project-slug 或 project ID', '例如 fabric-api；版本由 Minecraft/加载器筛选'],
      curseforge: ['CurseForge 项目 ID', '数字项目 ID', '例如 306612；需要在设置中配置 API Key'],
      spigot: ['Spigot 资源 ID', '数字资源 ID', '例如 9089；外部托管资源可能无法自动下载']
    };
    var meta = labels[type] || labels.github;
    els.taskSourceLabel.textContent = meta[0];
    els.taskRepo.placeholder = meta[1];
    els.taskSourceHint.textContent = meta[2];
    els.taskSourceError.textContent = isGithub ? '请输入 owner/repository 格式' : (isSpigot || isCurseforge ? '请输入数字 ID' : '请输入 Modrinth 项目标识');
    els.taskPlatformFilters.style.display = (isModrinth || isCurseforge) ? '' : 'none';
    document.getElementById('field-task-regex').style.display = isSpigot ? 'none' : '';
    els.taskPrereleaseRow.style.display = isSpigot ? 'none' : '';
  }

  function splitList(value) {
    var seen = {};
    return String(value || '').split(',').map(function (item) {
      return item.trim();
    }).filter(function (item) {
      if (!item) return false;
      var key = item.toLowerCase();
      if (seen[key]) return false;
      seen[key] = true;
      return true;
    });
  }

  function onTaskSubmit(event) {
    event.preventDefault();
    if (!store.config) {
      return;
    }
    clearFormError(els.taskFormError);
    var valid = true;
    var name = els.taskName.value.trim();
    var sourceType = els.taskSourceType.value || 'github';
    var source = els.taskRepo.value.trim();
    var regex = els.taskRegex.value.trim();
    var daemon = els.taskDaemon.value.trim();
    var instance = els.taskInstance.value.trim();
    var target = els.taskTarget.value.trim();
    var cleanupOld = els.taskCleanupOld.checked;
    var cleanupRegex = els.taskCleanupRegex.value.trim();
    var gameVersions = splitList(els.taskGameVersions.value);
    var loaders = splitList(els.taskLoaders.value);
    var category = getCategory();
    var editId = els.taskForm.getAttribute('data-edit-id') || '';
    var existing = editId ? findConfigTask(editId) : null;

    markField('field-task-name', !name);
    valid = Boolean(name) && valid;

    var sourceOk = true;
    if (sourceType === 'github') {
      var sourceParts = source.split('/');
      sourceOk = sourceParts.length === 2 && sourceParts[0].length > 0 && sourceParts[1].length > 0 && source.indexOf(' ') === -1;
    } else if (sourceType === 'modrinth') {
      sourceOk = source.length > 0 && source.indexOf('/') === -1 && source.indexOf(' ') === -1;
    } else {
      sourceOk = /^[0-9]+$/.test(source);
    }
    markField('field-task-repo', !sourceOk);
    valid = sourceOk && valid;

    var regexOk = sourceType === 'spigot' ? true : (sourceType === 'github' ? regex.length > 0 : true);
    if (regexOk && regex.length > 0) {
      try {
        new RegExp(regex);
      } catch (error) {
        regexOk = false;
      }
    }
    markField('field-task-regex', !regexOk);
    valid = regexOk && valid;

    var cleanupRegexOk = true;
    if (cleanupRegex.length > 0) {
      try {
        new RegExp(cleanupRegex);
      } catch (error) {
        cleanupRegexOk = false;
      }
    }
    markField('field-task-cleanup-regex', !cleanupRegexOk);
    valid = cleanupRegexOk && valid;

    markField('field-task-daemon', !daemon);
    markField('field-task-instance', !instance);
    valid = Boolean(daemon) && Boolean(instance) && valid;

    if (target.length > 0) {
      while (target.charAt(0) === '/') {
        target = target.slice(1);
      }
      target = '/' + target;
    }
    var targetTail = target.split('/').pop();
    var targetIsDir = target.endsWith('/') || (targetTail && targetTail.indexOf('.') === -1);
    var targetOk = target.length > 1 && target.indexOf('..') === -1;
    if (cleanupOld && !targetIsDir) {
      targetOk = false;
      showFormError(els.taskFormError, els.taskFormErrorText, '启用旧文件清理时，目标路径必须是目录（以 / 结尾），不能是单个文件。');
    }
    markField('field-task-target', !targetOk);
    valid = targetOk && valid;

    if (!valid) {
      if (!els.taskFormError.classList.contains('is-visible')) {
        showFormError(els.taskFormError, els.taskFormErrorText, '请修正表单中的高亮字段后再保存。');
      }
      return;
    }

    var task = {
      id: existing ? existing.id : '',
      name: name,
      enabled: els.taskEnabled.checked,
      source_type: sourceType,
      source: source,
      asset_regex: sourceType === 'spigot' ? '' : regex,
      include_prerelease: els.taskPrerelease.checked,
      game_versions: gameVersions,
      loaders: loaders,
      mcsm_daemon_id: daemon,
      mcsm_instance_uuid: instance,
      mcsm_target_path: target,
      restart: els.taskRestart.checked,
      cleanup_old: cleanupOld,
      cleanup_regex: cleanupRegex
    };
    if (category !== 'auto') {
      task.category = category;
    }
    if (existing && existing.restart_mode) {
      task.restart_mode = existing.restart_mode;
    }

    var tasks = store.config.tasks ? store.config.tasks.slice() : [];
    var replaced = false;
    for (var index = 0; index < tasks.length; index += 1) {
      if (existing && tasks[index].id === existing.id) {
        tasks[index] = task;
        replaced = true;
        break;
      }
    }
    if (!replaced) {
      tasks.push(task);
    }
    persistTaskForm(tasks);
  }

  async function persistTaskForm(tasks) {
    setButtonLoading(els.taskSubmitButton, true);
    try {
      var response = await api('/api/config', { method: 'POST', body: JSON.stringify(buildConfigPayload(tasks)) });
      store.config = response.config;
      store.meta = response.meta;
      setButtonLoading(els.taskSubmitButton, false);
      els.taskDialog.close();
      await refreshStatus();
      showSnackbar('任务配置已保存', '查看日志', openLogs);
    } catch (error) {
      setButtonLoading(els.taskSubmitButton, false);
      showFormError(els.taskFormError, els.taskFormErrorText, error.message);
    }
  }

  async function onTaskDelete() {
    var editId = els.taskForm.getAttribute('data-edit-id');
    if (!editId || !store.config) {
      return;
    }
    var task = findTask(editId);
    if (task && isTaskBusy(task)) {
      showSnackbar('任务正在执行，暂时不能删除', '关闭', null);
      return;
    }
    var confirmed = await showConfirm('删除更新任务', '将删除「' + (task ? task.name : editId) + '」的配置。已经部署到服务器的文件不会被删除。', '删除任务');
    if (!confirmed) {
      return;
    }
    var tasks = (store.config.tasks || []).filter(function (item) { return item.id !== editId; });
    setButtonLoading(els.taskSubmitButton, true);
    try {
      var response = await api('/api/config', { method: 'POST', body: JSON.stringify(buildConfigPayload(tasks)) });
      store.config = response.config;
      store.meta = response.meta;
      els.taskDialog.close();
      await refreshStatus();
      showSnackbar('任务已删除', '关闭', null);
    } catch (error) {
      setButtonLoading(els.taskSubmitButton, false);
      showFormError(els.taskFormError, els.taskFormErrorText, error.message);
    }
  }

  async function openSettings() {
    if (!store.config) {
      await loadConfig();
    }
    if (!store.config) {
      showSnackbar('配置尚未加载完成，请稍后重试', '关闭', null);
      return;
    }
    populateSettings();
    clearFormError(els.settingsFormError);
    els.settingsDialog.showModal();
  }

  function populateSettings() {
    var config = store.config;
    if (!config) {
      return;
    }
    els.settingsInterval.value = String(config.server.poll_interval_secs);
    els.settingsBaseUrl.value = config.mcsm.base_url || '';
    els.settingsApiKey.value = '';
    els.settingsDaemonScheme.value = config.mcsm.daemon_scheme || '';
    els.settingsTimeout.value = String(config.mcsm.timeout_secs);
    els.settingsApiInterval.value = String(config.mcsm.api_min_interval_ms == null ? 800 : config.mcsm.api_min_interval_ms);
    els.settingsApiRetries.value = String(config.mcsm.api_retry_attempts == null ? 4 : config.mcsm.api_retry_attempts);
    els.settingsUrlThreshold.value = String(config.mcsm.url_download_threshold_mb == null ? 8 : config.mcsm.url_download_threshold_mb);
    els.settingsRestartMode.value = config.mcsm.restart_mode || 'restart';
    els.settingsInsecure.checked = Boolean(config.mcsm.accept_invalid_certs);
    els.settingsGithubToken.value = '';
    els.settingsGithubApi.value = config.github.api_base || 'https://api.github.com';
    els.settingsCurseforgeKey.value = '';
    els.curseforgeKeyHint.textContent = config.providers && config.providers.curseforge_api_key_set
      ? '已配置 CurseForge Key，留空保持不变'
      : '仅 CurseForge 来源需要；留空保持现有 Key 不变';
    els.settingsCurseforgeKey.placeholder = config.providers && config.providers.curseforge_api_key_set
      ? '已配置，留空保持不变'
      : '输入 CurseForge API Key';
    els.settingsPath.textContent = store.meta ? store.meta.config_path : '—';
    els.apiKeyHint.textContent = config.mcsm.api_key_set ? '已配置密钥，留空保持不变' : '当前未配置；填写后才能执行上传';
    els.settingsApiKey.placeholder = config.mcsm.api_key_set ? '已配置，留空保持不变' : '输入 MCSM API Key';
    els.githubTokenHint.textContent = config.github.token_set ? '已配置 Token，留空保持不变' : '留空则匿名访问（60 次/小时）';
    els.settingsGithubToken.placeholder = config.github.token_set ? '已配置，留空保持不变' : 'ghp_…（可选）';
    Array.prototype.forEach.call(els.settingsForm.querySelectorAll('.field'), function (field) {
      field.classList.remove('has-error');
    });
    setButtonLoading(els.settingsSubmitButton, false);
  }

  function onSettingsSubmit(event) {
    event.preventDefault();
    if (!store.config) {
      return;
    }
    clearFormError(els.settingsFormError);
    var valid = true;
    var interval = Number(els.settingsInterval.value);
    var timeout = Number(els.settingsTimeout.value);
    var apiInterval = Number(els.settingsApiInterval.value);
    var apiRetries = Number(els.settingsApiRetries.value);
    var urlThreshold = Number(els.settingsUrlThreshold.value);
    var baseUrl = els.settingsBaseUrl.value.trim();
    var githubApi = els.settingsGithubApi.value.trim();

    var intervalOk = Number.isInteger(interval) && interval >= 30 && interval <= 86400;
    markField('field-settings-interval', !intervalOk);
    valid = intervalOk && valid;

    var timeoutOk = Number.isInteger(timeout) && timeout >= 10 && timeout <= 1800;
    markField('field-settings-timeout', !timeoutOk);
    valid = timeoutOk && valid;

    var apiIntervalOk = Number.isInteger(apiInterval) && apiInterval >= 0 && apiInterval <= 5000;
    markField('field-settings-api-interval', !apiIntervalOk);
    valid = apiIntervalOk && valid;

    var apiRetriesOk = Number.isInteger(apiRetries) && apiRetries >= 1 && apiRetries <= 10;
    markField('field-settings-api-retries', !apiRetriesOk);
    valid = apiRetriesOk && valid;

    var urlThresholdOk = Number.isInteger(urlThreshold) && urlThreshold >= 0 && urlThreshold <= 4096;
    markField('field-settings-url-threshold', !urlThresholdOk);
    valid = urlThresholdOk && valid;

    var baseUrlOk = isValidHttpUrl(baseUrl);
    markField('field-settings-baseurl', !baseUrlOk);
    valid = baseUrlOk && valid;

    var githubApiOk = isValidHttpUrl(githubApi);
    markField('field-settings-githubapi', !githubApiOk);
    valid = githubApiOk && valid;

    if (!valid) {
      showFormError(els.settingsFormError, els.settingsFormErrorText, '请修正表单中的高亮字段后再保存。');
      return;
    }

    var config = store.config;
    var payload = {
      server: {
        bind: config.server.bind,
        poll_interval_secs: interval,
        state_file: config.server.state_file,
        log_history: config.server.log_history
      },
      mcsm: {
        base_url: baseUrl,
        api_key: els.settingsApiKey.value.trim(),
        timeout_secs: timeout,
        restart_mode: els.settingsRestartMode.value,
        daemon_scheme: els.settingsDaemonScheme.value || null,
        accept_invalid_certs: els.settingsInsecure.checked,
        api_min_interval_ms: apiInterval,
        api_retry_attempts: apiRetries,
        url_download_threshold_mb: urlThreshold
      },
      github: {
        api_base: githubApi,
        token: els.settingsGithubToken.value.trim(),
        user_agent: config.github.user_agent
      },
      providers: {
        modrinth_base: config.providers.modrinth_base,
        curseforge_base: config.providers.curseforge_base,
        curseforge_api_key: els.settingsCurseforgeKey.value.trim(),
        spiget_base: config.providers.spiget_base,
        user_agent: config.providers.user_agent
      },
      tasks: config.tasks || []
    };
    saveSettings(payload);
  }

  async function saveSettings(payload) {
    setButtonLoading(els.settingsSubmitButton, true);
    try {
      var response = await api('/api/config', { method: 'POST', body: JSON.stringify(payload) });
      store.config = response.config;
      store.meta = response.meta;
      setButtonLoading(els.settingsSubmitButton, false);
      els.settingsDialog.close();
      await refreshStatus();
      showSnackbar('系统设置已保存并生效', '查看日志', openLogs);
    } catch (error) {
      setButtonLoading(els.settingsSubmitButton, false);
      showFormError(els.settingsFormError, els.settingsFormErrorText, error.message);
    }
  }

  function buildConfigPayload(tasks) {
    var config = store.config;
    return {
      server: {
        bind: config.server.bind,
        poll_interval_secs: config.server.poll_interval_secs,
        state_file: config.server.state_file,
        log_history: config.server.log_history
      },
      mcsm: {
        base_url: config.mcsm.base_url,
        api_key: '',
        timeout_secs: config.mcsm.timeout_secs,
        restart_mode: config.mcsm.restart_mode,
        daemon_scheme: config.mcsm.daemon_scheme || null,
        accept_invalid_certs: Boolean(config.mcsm.accept_invalid_certs),
        api_min_interval_ms: config.mcsm.api_min_interval_ms == null ? 800 : config.mcsm.api_min_interval_ms,
        api_retry_attempts: config.mcsm.api_retry_attempts == null ? 4 : config.mcsm.api_retry_attempts,
        url_download_threshold_mb: config.mcsm.url_download_threshold_mb == null ? 8 : config.mcsm.url_download_threshold_mb
      },
      github: {
        api_base: config.github.api_base,
        token: '',
        user_agent: config.github.user_agent
      },
      providers: {
        modrinth_base: config.providers.modrinth_base,
        curseforge_base: config.providers.curseforge_base,
        curseforge_api_key: '',
        spiget_base: config.providers.spiget_base,
        user_agent: config.providers.user_agent
      },
      tasks: tasks
    };
  }

  function isValidHttpUrl(value) {
    try {
      var parsed = new URL(value);
      return parsed.protocol === 'http:' || parsed.protocol === 'https:';
    } catch (error) {
      return false;
    }
  }

  function markField(id, hasError) {
    var field = document.getElementById(id);
    if (field) {
      field.classList.toggle('has-error', Boolean(hasError));
    }
  }

  function showFormError(container, textElement, message) {
    if (!container || !textElement) {
      return;
    }
    textElement.textContent = message;
    container.classList.add('is-visible');
  }

  function clearFormError(container) {
    if (container) {
      container.classList.remove('is-visible');
    }
  }

  function showConfirm(title, message, confirmLabel) {
    return new Promise(function (resolve) {
      if (store.confirmResolver) {
        var previous = store.confirmResolver;
        store.confirmResolver = null;
        previous(false);
      }
      store.confirmResolver = resolve;
      els.confirmTitle.textContent = title;
      els.confirmMessage.textContent = message;
      els.confirmAccept.textContent = confirmLabel || '确认';
      if (els.confirmDialog.open) {
        els.confirmDialog.close();
      }
      els.confirmDialog.showModal();
    });
  }

  function resolveConfirm(value) {
    var resolver = store.confirmResolver;
    store.confirmResolver = null;
    if (els.confirmDialog.open) {
      els.confirmDialog.close();
    }
    if (resolver) {
      resolver(value);
    }
  }

  function showSnackbar(message, actionLabel, action) {
    window.clearTimeout(store.snackTimer);
    els.snackbarText.textContent = message;
    store.snackAction = typeof action === 'function' ? action : null;
    if (store.snackAction && actionLabel) {
      els.snackbarAction.style.display = 'inline-flex';
      els.snackbarAction.textContent = actionLabel;
    } else {
      els.snackbarAction.style.display = 'none';
    }
    els.snackbar.classList.add('is-visible');
    store.snackTimer = window.setTimeout(hideSnackbar, 5600);
  }

  function hideSnackbar() {
    window.clearTimeout(store.snackTimer);
    els.snackbar.classList.remove('is-visible');
    store.snackAction = null;
  }

  function openLogs() {
    if (store.logCollapsed) {
      store.logCollapsed = false;
      applyLogState();
    }
    window.setTimeout(function () {
      els.logBody.scrollTop = els.logBody.scrollHeight;
    }, 220);
  }

  function restoreLogState() {
    try {
      store.logCollapsed = window.localStorage.getItem('mcsm.logCollapsed') === 'true';
    } catch (error) {
      store.logCollapsed = false;
    }
    applyLogState();
  }

  function toggleLogs() {
    store.logCollapsed = !store.logCollapsed;
    applyLogState();
    try {
      window.localStorage.setItem('mcsm.logCollapsed', store.logCollapsed ? 'true' : 'false');
    } catch (error) {
      // localStorage is optional in hardened browsers.
    }
  }

  function applyLogState() {
    els.logDock.classList.toggle('is-collapsed', store.logCollapsed);
    els.toggleLogsButton.setAttribute('aria-label', store.logCollapsed ? '展开日志面板' : '折叠日志面板');
    els.logToggleIcon.innerHTML = '<use href="#i-' + (store.logCollapsed ? 'chevron-up' : 'chevron-down') + '"></use>';
    syncLogMetrics();
  }

  function syncLogMetrics() {
    if (!els.logDock) {
      return;
    }
    var height = els.logDock.offsetHeight;
    document.documentElement.style.setProperty('--log-dock-height', height + 'px');
  }

  function clearLogs() {
    els.logBody.innerHTML = '<div class="log-empty" id="logEmpty">' + icon('terminal', 'icon-sm') + '等待后台日志事件…</div>';
    store.logCount = 0;
    els.logCount.textContent = '0 行';
  }

  function connectLogs() {
    if (store.source) {
      store.source.close();
    }
    var source = new EventSource('/api/logs/stream');
    store.source = source;
    source.onopen = function () {
      if (store.logWasConnected) {
        store.seenLogSeq.clear();
        store.seenLogQueue = [];
        els.logBody.innerHTML = '<div class="log-empty" id="logEmpty">' + icon('terminal', 'icon-sm') + '重新连接日志流，正在同步历史…</div>';
        store.logCount = 0;
        els.logCount.textContent = '0 行';
      }
      store.logWasConnected = true;
      els.logLive.classList.remove('is-offline');
      els.logLive.textContent = 'LIVE';
    };
    source.onerror = function () {
      els.logLive.classList.add('is-offline');
      els.logLive.textContent = 'RECONNECT';
    };
    source.onmessage = function (event) {
      try {
        appendLog(JSON.parse(event.data));
      } catch (error) {
        // Ignore malformed SSE frames.
      }
    };
  }

  function appendLog(entry) {
    if (!entry || typeof entry.message !== 'string') {
      return;
    }
    if (entry.seq) {
      if (store.seenLogSeq.has(entry.seq)) {
        return;
      }
      store.seenLogSeq.add(entry.seq);
      store.seenLogQueue.push(entry.seq);
      if (store.seenLogQueue.length > 3000) {
        var old = store.seenLogQueue.shift();
        store.seenLogSeq.delete(old);
      }
    }
    var empty = document.getElementById('logEmpty');
    if (empty && empty.parentNode) {
      empty.parentNode.removeChild(empty);
    }
    var body = els.logBody;
    var stick = body.scrollHeight - body.scrollTop - body.clientHeight < 48;
    var level = String(entry.level || 'INFO').toUpperCase();
    if (!/^(TRACE|DEBUG|INFO|WARN|ERROR)$/.test(level)) {
      level = 'INFO';
    }
    var line = document.createElement('div');
    line.className = 'log-line level-' + level;
    line.title = entry.target ? String(entry.target) : '';
    line.innerHTML = '<span class="log-time">' + esc(formatTime(entry.timestamp_ms)) + '</span>'
      + '<span class="log-level">' + level + '</span>'
      + '<span class="log-message">' + esc(entry.message) + '</span>';
    body.appendChild(line);
    while (body.childElementCount > 900) {
      body.removeChild(body.firstElementChild);
    }
    store.logCount += 1;
    els.logCount.textContent = store.logCount + ' 行';
    if (stick || store.logCount < 6) {
      body.scrollTop = body.scrollHeight;
    }
  }

})();