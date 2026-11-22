// ignore_for_file: deprecated_member_use
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../features/connection/connection_controller.dart';
import '../platform/home_transport.dart';
import '../features/admin/admin_screen.dart';
import '../features/listen/listen_screen.dart';
import '../features/listen/public_feed.dart';
import '../features/shows/shows_screen.dart';
import '../features/connection/connection_screen.dart';

class Tube2PodApp extends StatefulWidget {
  const Tube2PodApp({super.key});
  @override
  State<Tube2PodApp> createState() => _Tube2PodAppState();
}

class _Tube2PodAppState extends State<Tube2PodApp> {
  ThemeMode mode = ThemeMode.system;
  @override
  void initState() {
    super.initState();
    SharedPreferences.getInstance().then((p) {
      if (mounted) {
        setState(
          () =>
              mode = ThemeMode.values.byName(p.getString('theme') ?? 'system'),
        );
      }
    });
  }

  void setMode(ThemeMode value) {
    setState(() => mode = value);
    SharedPreferences.getInstance().then(
      (p) => p.setString('theme', value.name),
    );
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: 'Tube2Pod',
    themeMode: mode,
    theme: ThemeData(
      colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xffd46948)),
      useMaterial3: true,
    ),
    darkTheme: ThemeData(
      colorScheme: ColorScheme.fromSeed(
        seedColor: const Color(0xffd46948),
        brightness: Brightness.dark,
      ),
      useMaterial3: true,
    ),
    home: Home(mode: mode, onMode: setMode),
  );
}

class Home extends StatefulWidget {
  const Home({super.key, required this.mode, required this.onMode});
  final ThemeMode mode;
  final ValueChanged<ThemeMode> onMode;
  @override
  State<Home> createState() => _HomeState();
}

class _HomeState extends State<Home> {
  final connection = ConnectionController();
  final player = HomePlayer();
  int tab = 0;
  List<dynamic> shows = [];
  List<Map<String, dynamic>> episodes = [];
  List<Map<String, dynamic>> subscriptions = [];
  List<Map<String, dynamic>> subscriptionEpisodes = [];
  int subscriptionVersion = 0;
  List<Map<String, dynamic>> get visibleSubscriptions => subscriptions
      .where(
        (subscription) => !shows.any(
          (show) => show['public_feed_url'] == subscription['public_feed_url'],
        ),
      )
      .toList();
  List<Map<String, dynamic>> get visibleSubscriptionEpisodes =>
      subscriptionEpisodes
          .where(
            (episode) => visibleSubscriptions.any(
              (show) => show['id'] == episode['show_id'],
            ),
          )
          .toList();
  Map<String, dynamic>? selected;
  bool playing = false;
  double position = 0, duration = 0;
  Timer? timer;
  @override
  void initState() {
    super.initState();
    connection.addListener(changed);
    loadSubscriptions();
    connection.reconnect().then((_) => refresh()).catchError((Object e) {
      message(e);
      loadPublic();
    });
    timer = Timer.periodic(const Duration(seconds: 1), (_) async {
      if (!playing) return;
      try {
        final p = await player.position();
        if (mounted && p != null) {
          setState(() {
            position = ((p['position'] as num?) ?? 0).toDouble();
            duration = ((p['duration'] as num?) ?? 0).toDouble();
            playing = p['playing'] == true;
          });
        }
      } catch (_) {}
    });
  }

  void changed() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    timer?.cancel();
    player.close();
    connection.removeListener(changed);
    connection.dispose();
    super.dispose();
  }

  Future<void> refresh() async {
    if (connection.status != ConnectionStatus.lan &&
        connection.status != ConnectionStatus.remote) {
      await loadPublic();
      return;
    }
    try {
      final result = await connection.request('/v1/shows') as List;
      final all = <Map<String, dynamic>>[];
      for (final show in result) {
        final items = await connection.request(
          '/v1/shows/${show['id']}/episodes?status=published',
        ) as List;
        for (final item in items) {
          all.add({
            ...Map<String, dynamic>.from(item as Map),
            'show_title': show['title'],
          });
        }
      }
      final publicShows = result
          .where(
            (s) =>
                s['feed_enabled'] == true &&
                Uri.tryParse(s['public_feed_url']?.toString() ?? '')?.scheme ==
                    'https',
          )
          .map(
            (s) => {
              'id': s['id'],
              'title': s['title'],
              'description': s['description'],
              'feed_enabled': true,
              'public_feed_url': s['public_feed_url'],
            },
          )
          .toList();
      final prefs = await SharedPreferences.getInstance();
      await prefs.setString('public_shows', jsonEncode(publicShows));
      if (mounted) {
        setState(() {
          shows = result;
          episodes = all;
        });
      }
    } catch (e) {
      message(e);
      await loadPublic();
    }
  }

  Future<void> loadPublic() async {
    final prefs = await SharedPreferences.getInstance();
    final cached = jsonDecode(prefs.getString('public_shows') ?? '[]') as List;
    final all = <Map<String, dynamic>>[];
    for (final show in cached) {
      try {
        all.addAll(await readFeed(show['public_feed_url'] as String, show));
      } catch (_) {}
    }
    if (mounted) {
      setState(() {
        shows = cached;
        episodes = all;
      });
    }
  }

  Future<void> loadSubscriptions() async {
    final version = subscriptionVersion;
    final prefs = await SharedPreferences.getInstance();
    final urls = prefs.getStringList('subscribed_feed_urls') ?? [];
    final found = <Map<String, dynamic>>[];
    final all = <Map<String, dynamic>>[];
    for (final url in urls) {
      final show = <String, dynamic>{
        'id': url,
        'title': '',
        'feed_enabled': true,
        'public_feed_url': url,
      };
      try {
        all.addAll(await readFeed(url, show));
      } catch (_) {
        show['title'] = 'Không tải được feed';
      }
      found.add(show);
    }
    if (mounted) {
      setState(() {
        if (version == subscriptionVersion) {
          subscriptions = found;
          subscriptionEpisodes = all;
        } else {
          final current = prefs.getStringList('subscribed_feed_urls') ?? [];
          for (final show in found) {
            if (current.contains(show['id']) &&
                !subscriptions.any((s) => s['id'] == show['id'])) {
              subscriptions.add(show);
              subscriptionEpisodes.addAll(
                all.where((e) => e['show_id'] == show['id']),
              );
            }
          }
        }
      });
    }
  }

  Future<void> subscribe(String url) async {
    final uri = Uri.tryParse(url);
    if (uri == null ||
        uri.scheme != 'https' ||
        uri.host.isEmpty ||
        uri.userInfo.isNotEmpty) {
      throw const FormatException('Nhập link RSS HTTPS hợp lệ');
    }
    final prefs = await SharedPreferences.getInstance();
    final urls = prefs.getStringList('subscribed_feed_urls') ?? [];
    if (urls.contains(url)) {
      throw const FormatException('Show đã được thêm');
    }
    final show = <String, dynamic>{
      'id': url,
      'title': '',
      'feed_enabled': true,
      'public_feed_url': url,
    };
    final items = await readFeed(url, show);
    await prefs.setStringList('subscribed_feed_urls', [...urls, url]);
    subscriptionVersion++;
    if (mounted) {
      setState(() {
        subscriptions.add(show);
        subscriptionEpisodes.addAll(items);
      });
    }
  }

  void message(Object e) {
    if (mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(e.toString())));
    }
  }

  Future<void> play(Map<String, dynamic> ep) async {
    try {
      if (selected == ep) {
        if (playing) {
          await player.pause();
        } else {
          await player.play();
        }
        setState(() => playing = !playing);
        return;
      }
      final enclosure = ep['enclosure'] as String?;
      await player.open(
        enclosure != null
            ? HomeMediaSource.enclosure(Uri.parse(enclosure))
            : HomeMediaSource.paired(
                connection.endpoint(ep['media_path'] as String),
                headers: connection.headers,
              ),
      );
      setState(() {
        selected = ep;
        playing = true;
        position = 0;
        duration = 0;
      });
    } catch (e) {
      message(e);
    }
  }

  Future<void> scan() async {
    final raw = await Navigator.push<String>(
      context,
      MaterialPageRoute(builder: (_) => const Scanner()),
    );
    if (raw == null) return;
    try {
      await connection.pair(PairEnvelopeV1.parse(raw));
      await refresh();
    } catch (e) {
      message(e);
    }
  }

  Future<List<Map<String, dynamic>>> readFeed(String url, dynamic show) async {
    final uri = Uri.parse(url);
    if (uri.scheme != 'https') throw const FormatException('Feed cần HTTPS');
    final client = HttpClient();
    try {
      return await (() async {
        final request = await client.getUrl(uri);
        request.followRedirects = false;
        final response = await request.close();
        if (response.statusCode != 200) {
          throw StateError('HTTP ${response.statusCode}');
        }
        try {
          return parsePublicFeed(
            await utf8.decoder.bind(response).join(),
            show,
          );
        } on FormatException {
          throw const FormatException('Feed RSS không hợp lệ');
        }
      })().timeout(const Duration(seconds: 10));
    } finally {
      client.close(force: true);
    }
  }

  Widget episodeRow(Map<String, dynamic> ep) => ListenEpisodeRow(
    episode: ep,
    playing: selected == ep && playing,
    onPlay: () => play(ep),
  );
  @override
  Widget build(BuildContext context) {
    final online =
        connection.status == ConnectionStatus.lan ||
        connection.status == ConnectionStatus.remote;
    final status = switch (connection.status) {
      ConnectionStatus.lan => 'Trong mạng nhà',
      ConnectionStatus.remote => 'Từ xa',
      ConnectionStatus.revoked => 'Đã thu hồi',
      ConnectionStatus.disconnected => 'Kết nối',
    };
    return Scaffold(
      appBar: AppBar(
        title: const Text('Tube2Pod'),
        actions: [
          TextButton(
            onPressed: () => setState(() => tab = 3),
            child: Text(status),
          ),
          PopupMenuButton<ThemeMode>(
            tooltip: 'Giao diện',
            initialValue: widget.mode,
            onSelected: widget.onMode,
            itemBuilder: (_) => const [
              PopupMenuItem(
                value: ThemeMode.system,
                child: Text('Theo hệ thống'),
              ),
              PopupMenuItem(value: ThemeMode.light, child: Text('Sáng')),
              PopupMenuItem(value: ThemeMode.dark, child: Text('Tối')),
            ],
          ),
        ],
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(48),
          child: Row(
            children: [
              for (final (i, label) in ['Nghe', 'Show', 'Quản trị'].indexed)
                Expanded(
                  child: TextButton(
                    onPressed: () => setState(() => tab = i),
                    child: Text(
                      label,
                      style: TextStyle(
                        fontWeight: tab == i
                            ? FontWeight.bold
                            : FontWeight.normal,
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
      body: Column(
        children: [
          Expanded(
            child: ListView(
              padding: const EdgeInsets.all(20),
              children: [
                if (tab == 0) ...[
                  Text(
                    'Nghe podcast',
                    style: Theme.of(context).textTheme.headlineMedium,
                  ),
                  if (episodes.isEmpty && visibleSubscriptionEpisodes.isEmpty)
                    const Text('Chưa có tập được phát hành'),
                  ...episodes.map(episodeRow),
                  ...visibleSubscriptionEpisodes.map(episodeRow),
                ],
                if (tab == 1) ...[
                  Text(
                    'Show',
                    style: Theme.of(context).textTheme.headlineMedium,
                  ),
                  const Text('Theo dõi các show'),
                  Align(
                    alignment: Alignment.centerRight,
                    child: FilledButton(
                      onPressed: () => showDialog<void>(
                        context: context,
                        builder: (_) => AddRssDialog(subscribe: subscribe),
                      ),
                      child: const Text('Dán link RSS'),
                    ),
                  ),
                  ShowsGrid(
                    shows: [...shows, ...visibleSubscriptions],
                    cover: cover,
                    onOpen: showEpisodes,
                  ),
                ],
                if (tab == 2) ...[
                  Text(
                    'Quản trị',
                    style: Theme.of(context).textTheme.headlineMedium,
                  ),
                  if (!online)
                    TextButton(
                      onPressed: () => setState(() => tab = 3),
                      child: const Text('Mở Kết nối'),
                    )
                  else ...[
                    Align(
                      alignment: Alignment.centerRight,
                      child: FilledButton(
                        onPressed: () => editShow(null),
                        child: const Text('Tạo show'),
                      ),
                    ),
                    for (final show in shows)
                      ListTile(
                        leading: cover(show),
                        title: Text(show['title'].toString()),
                        subtitle: Text(
                          '${show['ready_count']} tập sẵn sàng · RSS ${show['feed_enabled'] == true ? 'đã bật' : 'đang tắt'}',
                        ),
                        onTap: () => manageShow(show),
                      ),
                  ],
                ],
                if (tab == 3) ...[
                  Text(
                    'Kết nối',
                    style: Theme.of(context).textTheme.headlineMedium,
                  ),
                  const Text('Ghép app với Tube2Pod trên máy chủ tại nhà'),
                  Card(
                    child: Padding(
                      padding: const EdgeInsets.all(16),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(status),
                          const Text(
                            'Mở Kết nối trên website máy chủ rồi tạo mã dùng một lần.',
                          ),
                          FilledButton(
                            onPressed: scan,
                            child: const Text('Quét QR'),
                          ),
                          TextButton(
                            onPressed: () async {
                              try {
                                await connection.reconnect();
                                await refresh();
                              } catch (e) {
                                message(e);
                              }
                            },
                            child: const Text('Thử kết nối lại'),
                          ),
                        ],
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
          if (selected != null)
            Column(
              children: [
                Slider(
                  value: position.clamp(0, duration > 0 ? duration : 1),
                  max: duration > 0 ? duration : 1,
                  onChanged: (v) => setState(() => position = v),
                  onChangeEnd: (v) =>
                      player.seek(Duration(milliseconds: v.round())),
                ),
                ListTile(
                  leading: selectedCover(),
                  title: Text(selected!['title']?.toString() ?? ''),
                  subtitle: Text(selected!['show_title']?.toString() ?? ''),
                  onTap: openPlayer,
                  trailing: IconButton(
                    onPressed: () => play(selected!),
                    icon: Icon(
                      playing ? Icons.pause_circle : Icons.play_circle,
                    ),
                  ),
                ),
              ],
            ),
        ],
      ),
    );
  }

  Widget cover(dynamic show) => show['public_cover'] != null
      ? Image.network(
          show['public_cover'] as String,
          width: 64,
          height: 64,
          fit: BoxFit.cover,
          errorBuilder: (_, _, _) => const Icon(Icons.album),
        )
      : show['cover_path'] == null || connection.currentUri == null
      ? const SizedBox(
          width: 64,
          height: 64,
          child: Icon(Icons.album, size: 48),
        )
      : FutureBuilder<HomeResponse>(
          future: connection.transport.request(
            connection.endpoint(show['cover_path'] as String),
            HomeRequest(headers: connection.headers),
          ),
          builder: (_, snapshot) =>
              snapshot.hasData && snapshot.data!.status == 200
              ? Image.memory(
                  snapshot.data!.body,
                  width: 64,
                  height: 64,
                  fit: BoxFit.cover,
                )
              : const SizedBox(width: 64, height: 64, child: Icon(Icons.album)),
        );
  Widget selectedCover() {
    final show = [
      ...shows,
      ...subscriptions,
    ].where((s) => s['id'] == selected?['show_id']).firstOrNull;
    return show == null ? const Icon(Icons.album) : cover(show);
  }

  void openPlayer() {
    final episode = selected!;
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => PlayerScreen(
          episode: episode,
          player: player,
          cover: selectedCover(),
          toggle: () => play(episode),
        ),
      ),
    );
  }

  Future<void> showEpisodes(dynamic show) async {
    var items = [
      ...episodes,
      ...subscriptionEpisodes,
    ].where((e) => e['show_id'] == show['id']).toList();
    if (show['feed_enabled'] == true && show['public_feed_url'] != null) {
      try {
        items = await readFeed(show['public_feed_url'] as String, show);
      } catch (_) {}
    }
    final prefs = await SharedPreferences.getInstance();
    final key = '${connection.serverId}:${show['id']}';
    final subscriptionUrl = show['id'] == show['public_feed_url']
        ? show['public_feed_url'] as String
        : null;
    var following =
        subscriptionUrl != null ||
        (prefs.getStringList('following') ?? []).contains(key);
    if (!mounted) return;
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => StatefulBuilder(
          builder: (context, update) => Scaffold(
            appBar: AppBar(title: Text(show['title'].toString())),
            body: ListView(
              children: [
                Text(show['description']?.toString() ?? ''),
                TextButton(
                  onPressed: () async {
                    if (subscriptionUrl != null) {
                      if (following) {
                        final urls =
                            prefs.getStringList('subscribed_feed_urls') ?? [];
                        urls.remove(subscriptionUrl);
                        await prefs.setStringList('subscribed_feed_urls', urls);
                        subscriptionVersion++;
                        if (mounted) {
                          setState(() {
                            subscriptions.removeWhere(
                              (s) => s['id'] == subscriptionUrl,
                            );
                            subscriptionEpisodes.removeWhere(
                              (e) => e['show_id'] == subscriptionUrl,
                            );
                          });
                        }
                      } else {
                        try {
                          await subscribe(subscriptionUrl);
                        } catch (e) {
                          message(e);
                          return;
                        }
                      }
                    } else {
                      final followed = prefs.getStringList('following') ?? [];
                      if (following) {
                        followed.remove(key);
                      } else {
                        followed.add(key);
                      }
                      await prefs.setStringList('following', followed);
                    }
                    if (context.mounted) update(() => following = !following);
                  },
                  child: Text(following ? 'Bỏ theo dõi' : 'Theo dõi'),
                ),
                ...items.map(episodeRow),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Future<void> manageShow(dynamic show) async {
    await Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => ManageShow(
          show: Map<String, dynamic>.from(show as Map),
          connection: connection,
          edit: editShow,
        ),
      ),
    );
    await refresh();
  }

  Future<void> editShow(dynamic show) async {
    await Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => ShowEditor(show: show, connection: connection),
      ),
    );
    await refresh();
  }
}
