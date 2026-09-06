Feature: set task

  Scenario: set builds an object
    Given a workflow with definition:
      """
      document: { dsl: '1.0.3', namespace: default, name: set, version: '1.0.0' }
      do:
        - build:
            set:
              value: 1
      """
    When the workflow is executed
    Then the workflow should complete with output:
      """
      value: 1
      """
