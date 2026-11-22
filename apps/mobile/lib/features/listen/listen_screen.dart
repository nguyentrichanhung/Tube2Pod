// ignore_for_file: deprecated_member_use
import 'dart:async';

import 'package:flutter/material.dart';

import '../../platform/home_transport.dart';

class ListenEpisodeRow extends StatelessWidget {
  const ListenEpisodeRow({
    super.key,
    required this.episode,
    required this.playing,
    required this.onPlay,
  });
  final Map<String, dynamic> episode;
  final bool playing;
  final VoidCallback onPlay;
  @override
  Widget build(BuildContext context) => ListTile(
    leading: const Icon(Icons.podcasts),
    title: Text(episode['title']?.toString() ?? ''),
    subtitle: Text(episode['show_title']?.toString() ?? ''),
    trailing: IconButton(
      onPressed: onPlay,
      icon: Icon(playing ? Icons.pause_circle : Icons.play_circle),
      tooltip: 'Phát',
    ),
  );
}

class PlayerScreen extends StatefulWidget {
  const PlayerScreen({
    super.key,
    required this.episode,
    required this.player,
    required this.cover,
    required this.toggle,
  });
  final Map<String, dynamic> episode;
  final HomePlayer player;
  final Widget cover;
  final Future<void> Function() toggle;
  @override
  State<PlayerScreen> createState() => _PlayerScreenState();
}

class _PlayerScreenState extends State<PlayerScreen> {
  Timer? timer;
  double position = 0, duration = 0;
  bool playing = false;
  @override
  void initState() {
    super.initState();
    update();
    timer = Timer.periodic(const Duration(seconds: 1), (_) => update());
  }

  Future<void> update() async {
    try {
      final p = await widget.player.position();
      if (mounted && p != null) {
        setState(() {
          position = ((p['position'] as num?) ?? 0).toDouble();
          duration = ((p['duration'] as num?) ?? 0).toDouble();
          playing = p['playing'] == true;
        });
      }
    } catch (_) {}
  }

  @override
  void dispose() {
    timer?.cancel();
    super.dispose();
  }

  String time(double millis) {
    final seconds = (millis / 1000).floor();
    return '${seconds ~/ 60}:${(seconds % 60).toString().padLeft(2, '0')}';
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('Trình phát')),
    body: ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text(
          widget.episode['title']?.toString() ?? '',
          style: Theme.of(context).textTheme.headlineMedium,
        ),
        Text(widget.episode['show_title']?.toString() ?? ''),
        const SizedBox(height: 24),
        Center(
          child: SizedBox(
            width: 240,
            height: 240,
            child: FittedBox(fit: BoxFit.cover, child: widget.cover),
          ),
        ),
        Slider(
          value: position.clamp(0, duration > 0 ? duration : 1),
          max: duration > 0 ? duration : 1,
          onChanged: (v) => setState(() => position = v),
          onChangeEnd: (v) =>
              widget.player.seek(Duration(milliseconds: v.round())),
        ),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [Text(time(position)), Text(time(duration))],
        ),
        FilledButton.icon(
          onPressed: () async {
            await widget.toggle();
            await update();
          },
          icon: Icon(playing ? Icons.pause : Icons.play_arrow),
          label: Text(playing ? 'Tạm dừng' : 'Nghe tập'),
        ),
        if (widget.episode['description'] != null)
          Text(widget.episode['description'].toString()),
      ],
    ),
  );
}
