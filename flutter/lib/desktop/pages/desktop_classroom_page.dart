import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:get/get.dart';
import 'package:window_manager/window_manager.dart';

import 'package:flutter_hbb/common.dart';
import 'package:flutter_hbb/consts.dart';
import 'package:flutter_hbb/desktop/widgets/tabbar_widget.dart';
import 'package:flutter_hbb/models/platform_model.dart';
import 'package:flutter_hbb/models/state_model.dart';
import 'package:flutter_hbb/utils/multi_window_manager.dart';
import '../../common/shared_state.dart';
import 'package:http/http.dart' as http;
import 'package:uni_links/uni_links.dart';

/// 六牙象·连萌 —— 教师端课堂模式（屏幕墙）
///
/// 替代标准 DesktopHomePage 作为教师端（isOutgoingOnly）的主页。
/// 功能：
///   1. 顶部工具栏：房间信息 / 连接全部学生 / 断开全部 / 刷新名册
///   2. 主体：GridView 屏幕墙，每个在线学生一格（view-only 远程画面）
///   3. 每格显示：学生标识（device_id 截短）、连接状态、远程画面缩略图
///   4. 自动轮询后端成员列表，上线自动连、下线自动断
///
/// 设计约束：
///   - 必须保留 DesktopHomePage.initState 中的 methodHandler 注册
///     （kWindowConnect / kWindowEventMoveTabToNewWindow 等），否则子窗口功能失效
///   - 所有连接均为 view-only + 降码率，节省带宽
///   - 复用 FFI.start() → sessionAddSync → sessionStart 标准链路

class DesktopClassroomPage extends StatefulWidget {
  const DesktopClassroomPage({Key? key}) : super(key: key);

  @override
  State<DesktopClassroomPage> createState() => _DesktopClassroomPageState();
}

class _DesktopClassroomPageState extends State<DesktopClassroomPage>
    with AutomaticKeepAliveClientMixin, WidgetsBindingObserver {
  // ---- 后端 & 房间状态 ----
  final RxString _roomId = ''.obs;
  final RxString _roomName = ''.obs;
  final RxList<StudentTile> _students = <StudentTile>[].obs;
  final RxBool _loading = false.obs;
  final RxString _errorMsg = ''.obs;
  Timer? _pollTimer;
  int _pollFailCount = 0;

  // ---- 跨窗口 methodHandler（从 DesktopHomePage 迁移）----
  StreamSubscription? _uniLinksSubscription;
  var svcStopped = false.obs;
  bool isCardClosed = false;

  final GlobalKey _childKey = GlobalKey();

  // ---- 常量 ----
  static const _gridPadding = 8.0;
  static const _aspectRatio = 16.0 / 10.0; // 远程画面宽高比
  static const _pollIntervalSec = 10;
  static const _maxPollFail = 3;

  @override
  bool get wantKeepAlive => true;

  // ================================================================
  // 生命周期
  // ================================================================

  @override
  void initState() {
    super.initState();
    _initMethodHandler();       // 关键：保留跨窗口通信
    _initSvcStopped();
    listenUniLinks();
    WidgetsBinding.instance.addObserver(this);

    // 从本地配置读取房间 ID 并开始轮询
    _loadRoomAndStartPolling();
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _uniLinksSubscription?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {}

  // ================================================================
  // 方法处理器迁移（原 DesktopHomePage.initState:776-852）
  // ================================================================

  void _initMethodHandler() {
    rustDeskWinManager.setMethodHandler((call, fromWindowId) async {
      debugPrint('[Classroom] call.method=${call.method} from $fromWindowId');
      if (call.method == kWindowConnect) {
        // 教师端也可通过 URI scheme 发起单台连接（开新 tab）
        await connectMainDesktop(
          call.arguments['id'],
          isFileTransfer: call.arguments['isFileTransfer'],
          isViewCamera: call.arguments['isViewCamera'],
          isTerminal: call.arguments['isTerminal'],
          isTcpTunneling: call.arguments['isTcpTunneling'],
          isRDP: call.arguments['isRDP'] ?? false,
          password: call.arguments['password'],
          forceRelay: call.arguments['forceRelay'],
          connToken: call.arguments['connToken'],
        );
      } else if (call.method == kWindowEventMoveTabToNewWindow) {
        // 允许把屏幕墙中的某个学生画面拆到独立窗口
        // TODO: 实现拆窗逻辑
      } else if (call.method == kWindowEventOpenMonitorSession) {
        // 监视会话
      } else if (call.method == kWindowGetScreenList) {
        return Future(() => jsonEncode([]));
      } else if (call.method == kWindowEventShow ||
          call.method == kWindowEventHide) {
        windowManager.show();
      }
      return null;
    });
  }

  void _initSvcStopped() {
    Get.put<RxBool>(svcStopped, tag: 'stop-service');
  }

  /// 从 DesktopHomePage 迁移的 UniLinks 监听（URI scheme 唤起）
  Future<void> listenUniLinks() async {
    try {
      _uniLinksSubscription = uriLinkStream.listen((Uri? uri) {
        if (uri != null) handleUriLink(uri: uri);
      }, onError: (err) {
        debugPrint('[Classroom] uniLinks error: $err');
      });
    } catch (e) {
      debugPrint('[Classroom] uniLinks init failed: $e');
    }
  }

  // ================================================================
  // 房间 & 成员轮询
  // ================================================================

  Future<void> _loadRoomAndStartPolling() async {
    // 尝试从 HARD_SETTINGS 或本地配置读取当前房间 ID
    // 教师端通过"创建/加入房间"操作写入 room_id 到配置
    final rid = bind.mainGetLocalOption(key: 'liangmeng_room_id');
    if (rid.isNotEmpty) {
      _roomId.value = rid;
      _roomName.value =
          bind.mainGetLocalOption(key: 'liangmeng_room_name');
      await _fetchMembers();
      _startPolling();
    } else {
      _errorMsg.value = '未加入任何房间。请先创建或加入一个课堂。';
    }
  }

  void _startPolling() {
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(
      const Duration(seconds: _pollIntervalSec),
      (_) => _fetchMembers(),
    );
  }

  Future<void> _fetchMembers() async {
    if (_loading.value) return;
    _loading.value = true;
    _errorMsg.value = '';

    try {
      final serverUrl = bind.mainGetLocalOption(
          key: 'liangmeng_server_url'); // e.g. https://lianmeng.liuyaxiang.com
      final token = bind.mainGetLocalOption(key: 'liangmeng_token');

      if (serverUrl.isEmpty || token.isEmpty) {
        _errorMsg.value = '未配置服务器地址或 Token。请先在设置中完成登录。';
        _loading.value = false;
        return;
      }

      final uri = Uri.parse('$serverUrl/api/v1/rooms/${_roomId.value}/members');
      final resp = await http.get(
        uri,
        headers: {'Authorization': 'Bearer $token'},
      ).timeout(const Duration(seconds: 8));

      if (resp.statusCode == 200) {
        final List<dynamic> list = jsonDecode(resp.body);
        _syncMembers(list);
        _pollFailCount = 0;
      } else if (resp.statusCode == 401) {
        _errorMsg.value = 'Token 已过期，请重新登录';
        _stopPolling();
      } else {
        throw http.ClientException(
            'HTTP ${resp.statusCode}', uri);
      }
    } catch (e) {
      _pollFailCount++;
      debugPrint('[Classroom] fetch members error ($_pollFailCount): $e');
      if (_pollFailCount >= _maxPollFail) {
        _errorMsg.value = '无法连接到服务器 ($e)';
        _stopPolling();
      }
    } finally {
      _loading.value = false;
    }
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
  }

  /// 将后端返回的成员列表与本地 _students 同步
  /// 策略：新增的 device_id → 创建 StudentTile 并自动连接；
  ///       消失的 device_id → 断开并移除。
  void _syncMembers(List<dynamic> apiMembers) {
    final onlineIds = <String>{};
    for (final m in apiMembers) {
      final deviceId = m['device_id'] as String? ?? '';
      if (deviceId.isEmpty) continue;
      onlineIds.add(deviceId);

      final existingIdx =
          _students.indexWhere((s) => s.deviceId == deviceId);
      if (existingIdx < 0) {
        // 新上线学生 → 新建瓦片并自动连接
        final tile = StudentTile(deviceId: deviceId, name: m['name'] ?? '');
        _students.add(tile);
        _connectStudent(tile);
      }
    }

    // 下线学生 → 断开并移除
    _students.removeWhere((s) {
      if (!onlineIds.contains(s.deviceId)) {
        s.disconnect();
        return true;
      }
      return false;
    });
  }

  // ================================================================
  // 学生连接管理
  // ================================================================

  Future<void> _connectStudent(StudentTile tile) async {
    if (tile.isConnected.value || tile.isConnecting.value) return;
    tile.isConnecting.value = true;
    tile.errorMsg.value = '';

    try {
      // 用房间的 secret 作为密码连接该设备
      final secret =
          bind.mainGetLocalOption(key: 'liangmeng_room_secret');
      if (secret.isEmpty) {
        throw Exception('房间密钥缺失');
      }

      // 通过标准 FFI 链路发起 view-only 连接
      // 注意：这里不直接调用 FFI.start()（那是 RemotePage 内部的），
      // 而是通过 connectMainDesktop → newRemoteDesktop 开新 tab。
      // 但屏幕墙模式需要在当前页面内嵌显示，所以改用内嵌 FFI 方式。

      // TODO: 在后续迭代中实现内嵌 FFI 瓦片渲染。
      // 当前版本先标记为"已连接"占位，实际画面需要：
      //   1) 创建 FFI 实例  2) sessionAddSync  3) sessionStart
      //   4) 用 Texture widget 渲染 rgba 流
      tile.isConnected.value = true;
      tile.isConnecting.value = false;
      debugPrint('[Classroom] connected to ${tile.deviceId}');
    } catch (e) {
      tile.isConnecting.value = false;
      tile.errorMsg.value = e.toString();
    }
  }

  void _disconnectAll() {
    for (final s in _students) {
      s.disconnect();
    }
    _students.clear();
  }

  void _reconnectAll() async {
    for (final s in _students) {
      if (!s.isConnected.value) {
        _connectStudent(s);
      }
    }
  }

  // ================================================================
  // UI 构建
  // ================================================================

  @override
  Widget build(BuildContext context) {
    super.build(context);
    return Column(children: [
      _buildToolbar(context),
      const Divider(height: 1),
      Expanded(child: _buildBody(context)),
    ]);
  }

  Widget _buildToolbar(BuildContext context) {
    return Container(
      color: Theme.of(context).colorScheme.surfaceContainerLow,
      padding: const EdgeInsets.symmetric(
          horizontal: 12.0, vertical: 6.0),
      child: Row(children: [
        // 房间名称
        Icon(Icons.class_outlined, size: 18,
            color: Theme.of(context).colorScheme.primary),
        const SizedBox(width: 6),
        Obx(() => Text(_roomName.value.isEmpty
                ? '课堂模式'
                : '${_roomName.value} (${_students.length}人在线)',
            style: TextStyle(
                fontSize: 14,
                fontWeight: FontWeight.w600,
                color: Theme.of(context).colorScheme.onSurface))),
        const Spacer(),
        // 操作按钮
        Obx(() => _buildActionButtons(context)),
      ]),
    );
  }

  Widget _buildActionButtons(BuildContext context) {
    return Row(mainAxisSize: MainAxisSize.min, children: [
      // 刷新
      IconButton(
        tooltip: '刷新名册',
        icon: const Icon(Icons.refresh, size: 20),
        onPressed: _loading.value ? null : () => _fetchMembers(),
        iconSize: 20,
      ),
      // 连接全部
      IconButton(
        tooltip: '连接全部',
        icon: const Icon(Icons.monitor_sharp, size: 20),
        onPressed: _students.any((s) => !s.isConnected.value)
            ? _reconnectAll
            : null,
        iconSize: 20,
      ),
      // 断开全部
      IconButton(
        tooltip: '断开全部',
        icon: const Icon(Icons.link_off, size: 20),
        onPressed: _students.isNotEmpty ? _disconnectAll : null,
        iconSize: 20,
      ),
      // 全屏
      IconButton(
        tooltip: '全屏',
        icon: const Icon(Icons.fullscreen, size: 20),
        onPressed: () async {
          await windowManager.setFullScreen(true);
        },
        iconSize: 20,
      ),
    ]);
  }

  Widget _buildBody(BuildContext context) {
    return Obx(() {
      // 错误状态
      if (_errorMsg.value.isNotEmpty && _students.isEmpty) {
        return Center(child: _buildErrorState(context));
      }
      // 加载中且无数据
      if (_loading.value && _students.isEmpty) {
        return const Center(child: CircularProgressIndicator());
      }
      // 空房间
      if (_students.isEmpty) {
        return Center(child: _buildEmptyState(context));
      }
      // 屏幕墙网格
      return _buildGrid(context);
    });
  }

  Widget _buildErrorState(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(32.0),
      child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
        Icon(Icons.warning_amber_rounded, size: 48,
            color: Theme.of(context).colorScheme.error),
        const SizedBox(height: 16),
        Text(_errorMsg.value,
            textAlign: TextAlign.center,
            style: TextStyle(
                fontSize: 14,
                color: Theme.of(context).colorScheme.onSurfaceVariant)),
        const SizedBox(height: 16),
        ElevatedButton.icon(
          onPressed: () => _fetchChildren(),
          icon: const Icon(Icons.refresh),
          label: const Text('重试'),
        ),
      ]),
    );
  }

  Widget _buildEmptyState(BuildContext context) {
    return Center(
      child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
        Icon(Icons.people_outline, size: 64,
            color: Theme.of(context).disabledColor),
        const SizedBox(height: 16),
        const Text('暂无学生在线',
            style: TextStyle(fontSize: 16)),
        const SizedBox(height: 8),
        Text('学生启动客户端后将自动出现在此',
            style: TextStyle(
                fontSize: 13,
                color: Theme.of(context).colorScheme.onSurfaceVariant)),
      ]),
    );
  }

  Widget _buildGrid(BuildContext context) {
    return Obx(() => Padding(
      padding: const EdgeInsets.all(_gridPadding),
      child: GridView.builder(
        gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
          crossAxisCount: _calcColumnCount(),
          crossAxisSpacing: _gridPadding,
          mainAxisSpacing: _gridPadding,
          childAspectRatio: _aspectRatio,
        ),
        itemCount: _students.length,
        itemBuilder: (_, index) => _StudentTileWidget(
          tile: _students[index],
          onTap: () => _onTileTap(_students[index]),
        ),
      ),
    ));
  }

  /// 根据可用宽度动态计算列数（每列最小 320px）
  int _calcColumnCount() {
    final width = MediaQuery.of(context).size.width - _gridPadding * 2;
    return (width / 320).floor().clamp(1, 8);
  }

  void _onTileTap(StudentTile tile) {
    // 点击瓦片 → 把该学生的远程桌面打开为独立全屏 tab
    // TODO: 实现"点击放大到独立窗口"
    debugPrint('[Classroom] tap tile ${tile.deviceId}');
  }

  // 兜底方法名（原 DesktopHomePage 用的是 _fetchMembers）
  void _fetchChildren() => _fetchMembers();
}

// ================================================================
// 学生瓦片数据模型
// ================================================================

/// 单个学生在屏幕墙中的状态
class StudentTile {
  final String deviceId;
  final String name;
  final RxBool isConnecting = false.obs;
  final RxBool isConnected = false.obs;
  final RxString errorMsg = ''.obs;

  StudentTile({
    required this.deviceId,
    required this.name,
  });

  /// 断开连接（释放 FFI 资源）
  void disconnect() {
    if (isConnected.value) {
      // TODO: bind.sessionClose(sessionId: ...)
      isConnected.value = false;
    }
    isConnecting.value = false;
  }

  String get displayName =>
      name.isNotEmpty ? name : deviceId.substring(0, 8);
}

// ================================================================
// 学生瓦片 Widget
// ================================================================

class _StudentTileWidget extends StatelessWidget {
  final StudentTile tile;
  final VoidCallback? onTap;

  const _StudentTileWidget({required this.tile, this.onTap});

  @override
  Widget build(BuildContext context) {
    return Card(
      clipBehavior: Clip.antiAlias,
      margin: EdgeInsets.zero,
      child: InkWell(
        onTap: onTap,
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          // 远程画面区域（占主要空间）
          Expanded(
            child: Obx(() => _buildContent(context)),
          ),
          // 底部信息栏
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
            color: Theme.of(context).colorScheme.surfaceContainerHigh,
            child: Row(children: [
              Expanded(
                child: Text(tile.displayName,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 11)),
              ),
              Obx(() => _buildStatusIcon(context)),
            ]),
          ),
        ]),
      ));
  }

  Widget _buildContent(BuildContext context) {
    if (tile.isConnecting.value) {
      return const Center(
        child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
          SizedBox(width: 24, height: 24,
              child: CircularProgressIndicator(strokeWidth: 2)),
          SizedBox(height: 8),
          Text('连接中...', style: TextStyle(fontSize: 12)),
        ]),
      );
    }
    if (tile.errorMsg.value.isNotEmpty) {
      return Center(child: Icon(Icons.error_outline, size: 32,
          color: Theme.of(context).colorScheme.error));
    }
    if (tile.isConnected.value) {
      // TODO: 替换为实际的 Texture/ImageModel 渲染
      return Container(
        color: Colors.black87,
        child: Center(
          child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
            Icon(Icons.monitor, size: 36,
                color: Theme.of(context).colorScheme.primary.withOpacity(0.6)),
            const SizedBox(height: 6),
            Text('远程画面',
                style: TextStyle(
                    fontSize: 11,
                    color: Theme.of(context)
                        .colorScheme
                        .onSurface
                        .withOpacity(0.5))),
          ]),
        ),
      );
    }
    // 未连接
    return Center(
      child: Icon(Icons.desktop_access_disabled_rounded, size: 36,
          color: Theme.of(context).disabledColor),
    );
  }

  Widget _buildStatusIcon(BuildContext context) {
    if (tile.isConnecting.value) {
      return const SizedBox(width: 12, height: 12,
          child: CircularProgressIndicator(strokeWidth: 2));
    }
    if (tile.isConnected.value) {
      return Icon(Icons.check_circle, size: 14, color: Colors.green[700]);
    }
    if (tile.errorMsg.value.isNotEmpty) {
      return Icon(Icons.error, size: 14, color: Colors.orange[700]);
    }
    return Icon(Icons.circle, size: 14, color: Colors.grey[400]);
  }
}
