// ignore_for_file: deprecated_member_use
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:image_picker/image_picker.dart';

import '../connection/connection_controller.dart';
import '../../platform/home_transport.dart';

class ManageShow extends StatefulWidget {
  const ManageShow({
    super.key,
    required this.show,
    required this.connection,
    required this.edit,
  });
  final Map<String, dynamic> show;
  final ConnectionController connection;
  final Future<void> Function(dynamic) edit;
  @override
  State<ManageShow> createState() => _ManageShowState();
}

class _ManageShowState extends State<ManageShow> {
  List<dynamic> ready = [];
  late Map<String, dynamic> show;
  @override
  void initState() {
    super.initState();
    show = widget.show;
    load();
  }

  Future<void> load() async {
    try {
      final s = await widget.connection.request(
        '/v1/shows/${show['id']}',
      ) as Map<String, dynamic>;
      final e = await widget.connection.request(
        '/v1/shows/${show['id']}/episodes?status=ready',
      ) as List;
      if (mounted) {
        setState(() {
          show = s;
          ready = e;
        });
      }
    } catch (e) {
      error(e);
    }
  }

  void error(Object e) {
    if (mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(e.toString())));
    }
  }

  Future<void> publish(dynamic ep) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (c) => AlertDialog(
        title: const Text('Phát hành tập'),
        content: Text(
          'Phát hành ${ep['title']} trong ${show['title']}.${show['feed_enabled'] == true ? '' : ' Feed của show đang tắt.'}',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(c, false),
            child: const Text('Hủy'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(c, true),
            child: const Text('Phát hành'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    try {
      await widget.connection.request(
        '/v1/episodes/${ep['id']}/publish',
        method: 'POST',
      );
    } catch (e) {
      try {
        final result =
            await widget.connection.request('/v1/episodes/${ep['id']}') as Map;
        if (result['status'] != 'published') error(e);
      } catch (_) {
        error(e);
      }
    }
    await load();
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text(show['title'].toString())),
    body: ListView(
      padding: const EdgeInsets.all(20),
      children: [
        TextButton(
          onPressed: () async {
            await widget.edit(show);
            await load();
          },
          child: const Text('Sửa thông tin và ảnh bìa'),
        ),
        Text('Tập sẵn sàng', style: Theme.of(context).textTheme.titleLarge),
        if (ready.isEmpty) const Text('Chưa có tập sẵn sàng'),
        for (final ep in ready)
          ListTile(
            title: Text(ep['title'].toString()),
            trailing: FilledButton(
              onPressed: () => publish(ep),
              child: const Text('Phát hành'),
            ),
          ),
        SwitchListTile(
          title: const Text('Feed RSS'),
          value: show['feed_enabled'] == true,
          onChanged: (v) async {
            if (!v) {
              final confirm = await showDialog<bool>(
                context: context,
                builder: (c) => AlertDialog(
                  title: const Text('Tắt feed RSS'),
                  content: const Text(
                    'Người nghe sẽ mất quyền truy cập vào feed này.',
                  ),
                  actions: [
                    TextButton(
                      onPressed: () => Navigator.pop(c, false),
                      child: const Text('Hủy'),
                    ),
                    FilledButton(
                      onPressed: () => Navigator.pop(c, true),
                      child: const Text('Tắt feed'),
                    ),
                  ],
                ),
              );
              if (confirm != true) return;
            }
            try {
              await widget.connection.request(
                '/v1/shows/${show['id']}/feed',
                method: 'PUT',
                body: {'enabled': v},
              );
              await load();
            } catch (e) {
              error(e);
            }
          },
        ),
        if (show['feed_enabled'] == true &&
            show['public_feed_url'] != null) ...[
          SelectableText(show['public_feed_url'].toString()),
          TextButton(
            onPressed: () async {
              await Clipboard.setData(
                ClipboardData(text: show['public_feed_url'].toString()),
              );
              if (context.mounted) {
                ScaffoldMessenger.of(context).showSnackBar(
                  const SnackBar(content: Text('Đã sao chép đường dẫn RSS')),
                );
              }
            },
            child: const Text('Sao chép RSS'),
          ),
        ],
      ],
    ),
  );
}

class ShowEditor extends StatefulWidget {
  const ShowEditor({super.key, required this.show, required this.connection});
  final dynamic show;
  final ConnectionController connection;
  @override
  State<ShowEditor> createState() => _ShowEditorState();
}

class _ShowEditorState extends State<ShowEditor> {
  final fields = <String, TextEditingController>{};
  XFile? cover;
  bool saving = false;
  dynamic savedId;
  @override
  void initState() {
    super.initState();
    for (final key in [
      'title',
      'author',
      'description',
      'category',
      'language',
    ]) {
      fields[key] = TextEditingController(
        text:
            widget.show?[key]?.toString() ??
            (key == 'language'
                ? 'vi'
                : key == 'category'
                ? 'Technology'
                : ''),
      );
    }
  }

  @override
  void dispose() {
    for (final c in fields.values) {
      c.dispose();
    }
    super.dispose();
  }

  Future<void> save() async {
    setState(() => saving = true);
    try {
      final id = savedId ?? widget.show?['id'];
      final data = await widget.connection.request(
        id == null ? '/v1/shows' : '/v1/shows/$id',
        method: id == null ? 'POST' : 'PATCH',
        body: fields.map((k, v) => MapEntry(k, v.text)),
      ) as Map;
      savedId = data['id'];
      if (cover != null) {
        final boundary = 'tube2pod-boundary';
        final bytes = await cover!.readAsBytes();
        final body = Uint8List.fromList([
          ...utf8.encode(
            '--$boundary\r\nContent-Disposition: form-data; name="cover"; filename="cover"\r\nContent-Type: application/octet-stream\r\n\r\n',
          ),
          ...bytes,
          ...utf8.encode('\r\n--$boundary--\r\n'),
        ]);
        final response = await widget.connection.transport.request(
          widget.connection.endpoint('/v1/shows/${data['id']}/cover'),
          HomeRequest(
            method: 'POST',
            headers: {
              ...widget.connection.headers,
              'Content-Type': 'multipart/form-data; boundary=$boundary',
            },
            body: body,
          ),
        );
        if (response.status != 200) throw StateError('HTTP ${response.status}');
      }
      if (mounted) Navigator.pop(context);
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text(e.toString())));
      }
    } finally {
      if (mounted) setState(() => saving = false);
    }
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: Text(widget.show == null ? 'Tạo show' : 'Sửa show')),
    body: ListView(
      padding: const EdgeInsets.all(20),
      children: [
        for (final (key, label) in {
          'title': 'Tên show',
          'author': 'Tác giả',
          'description': 'Mô tả',
          'category': 'Danh mục',
          'language': 'Ngôn ngữ',
        }.entries.map((e) => (e.key, e.value)))
          TextField(
            controller: fields[key],
            decoration: InputDecoration(labelText: label),
          ),
        TextButton(
          onPressed: () async {
            final file = await ImagePicker().pickImage(
              source: ImageSource.gallery,
            );
            if (mounted && file != null) setState(() => cover = file);
          },
          child: Text(cover == null ? 'Chọn ảnh bìa' : cover!.name),
        ),
        FilledButton(onPressed: saving ? null : save, child: const Text('Lưu')),
      ],
    ),
  );
}
