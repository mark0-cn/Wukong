"""Protect the asynchronous version-merge handshake from stale/untrusted runs."""

import json
import unittest
from unittest.mock import Mock, patch
from scripts.release.request_release import request_release
from scripts.release.tag_release import wait_for_ci


class ReleaseRequestTests(unittest.TestCase):
    def setUp(self):
        self.sha = "a" * 40
        self.run = dict(
            event="push",
            head_branch="master",
            path=".github/workflows/ci.yml",
            repository=dict(full_name="owner/repo"),
            head_repository=dict(full_name="owner/repo"),
            head_sha=self.sha,
            run_attempt=2,
            status="in_progress",
            conclusion=None,
        )

    def test_only_a_verified_version_merge_dispatches_and_binds_source_attempt(self):
        with (
            patch("scripts.release.request_release.api", return_value=self.run),
            patch(
                "scripts.release.request_release.discover_candidate", return_value=None
            ) as candidate,
            patch("scripts.release.request_release.subprocess.run") as dispatch,
        ):
            self.assertFalse(request_release("owner/repo", 123, self.sha, 2))
            dispatch.assert_not_called()
            candidate.return_value = dict(tag="v1.2.3", sha=self.sha)
            self.assertTrue(request_release("owner/repo", 123, self.sha, 2))
            payload = json.loads(dispatch.call_args.kwargs["input"])
            self.assertEqual(
                payload,
                dict(
                    ref="master",
                    inputs=dict(
                        operation="tag", ci_run_id="123", ci_sha=self.sha, ci_run_attempt="2"
                    ),
                ),
            )

    def test_untrusted_or_stale_request_never_dispatches(self):
        for changes in (
            dict(event="pull_request"),
            dict(head_branch="topic"),
            dict(run_attempt=3),
            dict(head_sha="b" * 40),
            dict(path=".github/workflows/release.yml"),
        ):
            with (
                self.subTest(changes=changes),
                patch(
                    "scripts.release.request_release.api", return_value=dict(self.run, **changes)
                ),
                patch("scripts.release.request_release.discover_candidate") as candidate,
                patch("scripts.release.request_release.subprocess.run") as dispatch,
            ):
                with self.assertRaises(ValueError):
                    request_release("owner/repo", 123, self.sha, 2)
                candidate.assert_not_called()
                dispatch.assert_not_called()

    def test_receiver_waits_for_completed_success_without_sender_waiting(self):
        request = Mock(
            side_effect=[self.run, dict(self.run, status="completed", conclusion="success")]
        )
        sleep = Mock()
        wait_for_ci(123, "owner/repo", self.sha, 2, request=request, sleep=sleep)
        self.assertEqual(request.call_count, 2)
        sleep.assert_called_once_with(10)

    def test_failed_cancelled_stale_and_untrusted_sources_are_rejected(self):
        for changes in (
            dict(status="completed", conclusion="failure"),
            dict(status="completed", conclusion="cancelled"),
            dict(status="completed", conclusion="skipped"),
            dict(run_attempt=3),
            dict(head_sha="b" * 40),
            dict(head_repository=dict(full_name="fork/repo")),
        ):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                wait_for_ci(
                    123,
                    "owner/repo",
                    self.sha,
                    2,
                    request=Mock(return_value=dict(self.run, **changes)),
                    sleep=Mock(),
                )

    def test_wait_is_bounded_and_does_not_treat_timeout_as_success(self):
        request, sleep = Mock(return_value=self.run), Mock()
        with self.assertRaises(TimeoutError):
            wait_for_ci(123, "owner/repo", self.sha, 2, request=request, sleep=sleep, polls=3)
        self.assertEqual(request.call_count, 3)
        self.assertEqual(sleep.call_count, 2)
