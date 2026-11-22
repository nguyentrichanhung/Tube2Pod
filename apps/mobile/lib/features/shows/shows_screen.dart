// ignore_for_file: deprecated_member_use
import 'package:flutter/material.dart';

class ShowsGrid extends StatelessWidget {
  const ShowsGrid({
    super.key,
    required this.shows,
    required this.cover,
    required this.onOpen,
  });
  final List<dynamic> shows;
  final Widget Function(dynamic) cover;
  final void Function(dynamic) onOpen;
  @override
  Widget build(BuildContext context) => GridView.count(
    crossAxisCount: 2,
    shrinkWrap: true,
    physics: const NeverScrollableScrollPhysics(),
    children: [
      for (final show in shows)
        Card(
          child: InkWell(
            onTap: () => onOpen(show),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [cover(show), Text(show['title'].toString())],
            ),
          ),
        ),
    ],
  );
}

class AddRssDialog extends StatefulWidget {
  const AddRssDialog({super.key, required this.subscribe});
  final Future<void> Function(String) subscribe;
  @override
  State<AddRssDialog> createState() => _AddRssDialogState();
}

class _AddRssDialogState extends State<AddRssDialog> {
  final controller = TextEditingController();
  String? error;
  bool loading = false;
  @override
  void dispose() {
    controller.dispose();
    super.dispose();
  }

  Future<void> add() async {
    setState(() {
      loading = true;
      error = null;
    });
    try {
      await widget.subscribe(controller.text.trim());
      if (mounted) Navigator.pop(context);
    } on FormatException catch (e) {
      if (mounted) setState(() => error = e.message);
    } catch (_) {
      if (mounted) setState(() => error = 'Không tải được feed RSS');
    } finally {
      if (mounted) setState(() => loading = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Dán link RSS'),
    content: TextField(
      controller: controller,
      autofocus: true,
      keyboardType: TextInputType.url,
      decoration: InputDecoration(
        labelText: 'Link RSS HTTPS',
        errorText: error,
      ),
    ),
    actions: [
      TextButton(
        onPressed: loading ? null : () => Navigator.pop(context),
        child: const Text('Hủy'),
      ),
      FilledButton(
        onPressed: loading ? null : add,
        child: const Text('Thêm show'),
      ),
    ],
  );
}
